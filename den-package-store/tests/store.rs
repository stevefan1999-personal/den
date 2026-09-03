use den_package_store::{
    DependencyKind, NewDependency, NewExport, NewPackageFile, NewRelease, PackageStore,
    PackageStoreError, RegistryId, RootRequirement,
};
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn creates_migrates_and_reopens_store() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("packages.sqlite3");
    let store = PackageStore::create(&path)?;
    let registry_id = store.add_registry("npm", "https://registry.example/")?;
    drop(store);

    let reopened = PackageStore::open(&path)?;
    assert_eq!(
        reopened.registry_id("npm", "https://registry.example/")?,
        Some(registry_id)
    );
    drop(reopened);

    let database = Connection::open(&path)?;
    assert_eq!(pragma::<i64>(&database, "application_id")?, 0x4445_4e50);
    assert_eq!(pragma::<i64>(&database, "user_version")?, 1);
    assert_eq!(pragma::<String>(&database, "journal_mode")?, "wal");
    let non_strict: i64 = database.query_row(
        "SELECT COUNT(*) FROM pragma_table_list WHERE schema = 'main' AND name NOT LIKE \
         'sqlite_%' AND strict = 0",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(non_strict, 0);
    Ok(())
}

#[test]
fn relative_store_paths_create_and_reopen() -> TestResult {
    let directory = tempfile::tempdir_in(".")?;
    let current = std::env::current_dir()?;
    let path = directory
        .path()
        .join("relative.sqlite3")
        .strip_prefix(&current)?
        .to_path_buf();
    assert!(path.is_relative());
    let store = PackageStore::create(&path)?;
    let registry = store.add_registry("jsr", "https://jsr.example/")?;
    drop(store);
    assert_eq!(
        PackageStore::open(&path)?.registry_id("jsr", "https://jsr.example/")?,
        Some(registry)
    );
    Ok(())
}

#[test]
fn open_does_not_create_a_missing_store() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("missing.sqlite3");
    assert!(PackageStore::open(&path).is_err());
    assert!(!path.exists());
    Ok(())
}

#[test]
fn rejects_newer_schema() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("future.sqlite3");
    drop(PackageStore::create(&path)?);
    Connection::open(&path)?.pragma_update(None, "user_version", 2)?;

    assert!(matches!(
        PackageStore::open(&path),
        Err(PackageStoreError::SchemaTooNew {
            actual:    2,
            supported: 1,
        })
    ));
    Ok(())
}

#[test]
fn rejects_unknown_migration() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("unknown-migration.sqlite3");
    drop(PackageStore::create(&path)?);
    Connection::open(&path)?.execute_batch(
        "INSERT INTO den_package_store_migrations(version, applied_at) VALUES \
         ('m99999999_999999_future', 0)",
    )?;

    assert!(matches!(
        PackageStore::open(&path),
        Err(PackageStoreError::UnknownMigration(version))
            if version == "m99999999_999999_future"
    ));
    Ok(())
}

#[test]
fn rejects_and_preserves_foreign_application_identity() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("foreign.sqlite3");
    Connection::open(&path)?.pragma_update(None, "application_id", 305_419_896)?;

    assert!(matches!(
        PackageStore::open(&path),
        Err(PackageStoreError::ForeignDatabase {
            actual: 305_419_896,
        })
    ));
    assert_eq!(
        pragma::<i64>(&Connection::open(&path)?, "application_id")?,
        305_419_896
    );
    Ok(())
}

#[test]
fn blobs_deduplicate() -> TestResult {
    let store = PackageStore::open_in_memory()?;
    let digest = store.insert_blob(b"original")?;
    assert_eq!(store.insert_blob(b"original")?, digest);
    Ok(())
}

#[test]
fn registries_are_canonical_and_reject_embedded_credentials() -> TestResult {
    let store = PackageStore::open_in_memory()?;
    let first = store.add_registry("jsr", "https://registry.example/api")?;
    let second = store.add_registry("jsr", "https://registry.example/api/")?;
    assert_eq!(first, second);
    assert!(matches!(
        store.add_registry("jsr", "https://token@registry.example/"),
        Err(PackageStoreError::InvalidRegistry(_))
    ));
    Ok(())
}

#[test]
fn registry_lookup_does_not_create_missing_rows() -> TestResult {
    let store = PackageStore::open_in_memory()?;
    assert_eq!(store.registry_id("jsr", "https://jsr.example/")?, None);
    let id = store.add_registry("jsr", "https://jsr.example/")?;
    assert_eq!(store.registry_id("jsr", "https://jsr.example")?, Some(id));
    assert!(store.registry_id("npm", "https://jsr.example/").is_err());
    Ok(())
}

#[test]
fn concurrent_stores_insert_distinct_versions_of_one_new_package() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("concurrent.sqlite3");
    let first = PackageStore::create(&path)?;
    let registry = first.add_registry("jsr", "https://jsr.example/")?;
    let second = PackageStore::open(&path)?;
    let one = NewRelease::new(registry, "shared", "1.0.0");
    let two = NewRelease::new(registry, "shared", "2.0.0");

    let (one, two) = std::thread::scope(|scope| {
        let one = scope.spawn(|| first.insert_release(&one));
        let two = second.insert_release(&two);
        (one.join().expect("insert thread panicked"), two)
    });
    one?;
    two?;
    let snapshot = first.repository_snapshot()?;
    for version in ["1.0.0", "2.0.0"] {
        let solved = snapshot.solve(&[RootRequirement::new(registry, "shared", version)])?;
        assert_eq!(versions(&solved.packages), vec![("shared", version)]);
    }
    Ok(())
}

#[test]
fn invalid_release_leaves_no_partial_package() -> TestResult {
    let store = PackageStore::open_in_memory()?;
    let registry_id = store.add_registry("jsr", "https://jsr.example/")?;
    let digest = store.insert_blob(b"export default 1")?;
    let mut release = NewRelease::new(registry_id, "@scope/pkg", "1.0.0");
    release.files.push(NewPackageFile {
        path:       "../escape.ts".to_owned(),
        blob:       digest,
        media_type: Some("text/typescript".to_owned()),
        mode:       0o644,
    });

    assert!(matches!(
        store.insert_release(&release),
        Err(PackageStoreError::InvalidModulePath { .. })
    ));
    assert_package_absent(&store, registry_id, "@scope/pkg")
}

/// A failed release must leave nothing the solver can select.
fn assert_package_absent(store: &PackageStore, registry: RegistryId, name: &str) -> TestResult {
    let solved = store
        .repository_snapshot()?
        .solve(&[RootRequirement::new(registry, name, "*")]);
    assert!(
        matches!(solved, Err(PackageStoreError::Conflict(_))),
        "`{name}` must not be selectable after a failed release: {solved:?}"
    );
    Ok(())
}

#[test]
fn dangling_export_is_rejected_before_commit() -> TestResult {
    let store = PackageStore::open_in_memory()?;
    let registry_id = store.add_registry("jsr", "https://jsr.example/")?;
    let mut release = NewRelease::new(registry_id, "@scope/pkg", "1.0.0");
    release.exports.push(NewExport {
        name:   ".".to_owned(),
        target: "missing.ts".to_owned(),
    });

    assert!(matches!(
        store.insert_release(&release),
        Err(PackageStoreError::MissingExportTarget { .. })
    ));
    assert_package_absent(&store, registry_id, "@scope/pkg")
}

#[test]
fn solver_selects_highest_compatible_transitive_version() -> TestResult {
    let (store, registry_id) = store_with_registry()?;
    insert_release(
        &store,
        registry_id,
        "app",
        "1.0.0",
        &[("dep", "^1.0.0")],
        None,
    )?;
    insert_release(&store, registry_id, "dep", "1.0.0", &[], None)?;
    insert_release(&store, registry_id, "dep", "1.8.0", &[], None)?;
    insert_release(&store, registry_id, "dep", "2.0.0", &[], None)?;

    let solved =
        store
            .repository_snapshot()?
            .solve(&[RootRequirement::new(registry_id, "app", "*")])?;
    assert_eq!(versions(&solved.packages), vec![
        ("app", "1.0.0"),
        ("dep", "1.8.0")
    ]);
    Ok(())
}

#[test]
fn solver_reports_conflicts_and_excludes_yanked_versions() -> TestResult {
    let (store, registry_id) = store_with_registry()?;
    insert_release(
        &store,
        registry_id,
        "left",
        "1.0.0",
        &[("dep", "^1.0.0")],
        None,
    )?;
    insert_release(
        &store,
        registry_id,
        "right",
        "1.0.0",
        &[("dep", "^2.0.0")],
        None,
    )?;
    insert_release(
        &store,
        registry_id,
        "dep",
        "1.9.0",
        &[],
        Some("bad archive"),
    )?;
    insert_release(&store, registry_id, "dep", "1.8.0", &[], None)?;
    insert_release(&store, registry_id, "dep", "2.0.0", &[], None)?;

    let snapshot = store.repository_snapshot()?;
    let selected = snapshot.solve(&[RootRequirement::new(registry_id, "left", "*")])?;
    assert_eq!(versions(&selected.packages), vec![
        ("dep", "1.8.0"),
        ("left", "1.0.0")
    ]);

    let error = snapshot
        .solve(&[
            RootRequirement::new(registry_id, "left", "*"),
            RootRequirement::new(registry_id, "right", "*"),
        ])
        .err()
        .ok_or("expected an unsatisfiable result")?;
    let message = error.to_string();
    assert!(message.contains("dep"));
    assert!(message.contains("1.0.0") || message.contains("2.0.0"));
    Ok(())
}

#[test]
fn repeated_solves_are_deterministic() -> TestResult {
    let (store, registry_id) = store_with_registry()?;
    insert_release(&store, registry_id, "app", "1.0.0", &[("dep", "*")], None)?;
    insert_release(&store, registry_id, "app", "1.1.0", &[("dep", "^1")], None)?;
    insert_release(&store, registry_id, "dep", "1.0.0", &[], None)?;
    insert_release(&store, registry_id, "dep", "1.1.0", &[], None)?;
    let snapshot = store.repository_snapshot()?;
    let roots = [RootRequirement::new(registry_id, "app", "*")];
    let expected = snapshot.solve(&roots)?;
    for _ in 0..20 {
        assert_eq!(snapshot.solve(&roots)?, expected);
    }
    Ok(())
}

#[test]
fn flat_solver_rejects_optional_or_peer_semantics_instead_of_lying() -> TestResult {
    let (store, registry_id) = store_with_registry()?;
    let mut release = NewRelease::new(registry_id, "app", "1.0.0");
    release.dependencies.push(NewDependency {
        kind:               DependencyKind::Optional,
        target_registry_id: None,
        package:            "optional-dep".to_owned(),
        requirement:        "^1".to_owned(),
        alias:              None,
    });
    store.insert_release(&release)?;

    let error = store
        .repository_snapshot()?
        .solve(&[RootRequirement::new(registry_id, "app", "*")])
        .expect_err("flat solver must exclude optional semantics");
    assert!(matches!(error, PackageStoreError::Conflict(_)));
    assert!(error.to_string().contains("optional"));
    Ok(())
}

#[test]
fn flat_solver_excludes_dependency_aliases_instead_of_dropping_them() -> TestResult {
    let (store, registry_id) = store_with_registry()?;
    let mut release = NewRelease::new(registry_id, "app", "1.0.0");
    release.dependencies.push(NewDependency {
        kind:               DependencyKind::Normal,
        target_registry_id: None,
        package:            "real-name".to_owned(),
        requirement:        "^1".to_owned(),
        alias:              Some("alias-name".to_owned()),
    });
    store.insert_release(&release)?;

    let error = store
        .repository_snapshot()?
        .solve(&[RootRequirement::new(registry_id, "app", "*")])
        .expect_err("flat solver must exclude aliases");
    assert!(error.to_string().contains("alias-name"));
    Ok(())
}

fn store_with_registry() -> Result<(PackageStore, RegistryId), Box<dyn std::error::Error>> {
    let store = PackageStore::open_in_memory()?;
    let registry_id = store.add_registry("npm", "https://registry.example/")?;
    Ok((store, registry_id))
}

fn insert_release(
    store: &PackageStore, registry_id: RegistryId, package: &str, version: &str,
    dependencies: &[(&str, &str)], yanked_reason: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut release = NewRelease::new(registry_id, package, version);
    release.yanked_reason = yanked_reason.map(str::to_owned);
    release.dependencies = dependencies
        .iter()
        .map(|(package, requirement)| {
            NewDependency {
                kind:               DependencyKind::Normal,
                target_registry_id: None,
                package:            (*package).to_owned(),
                requirement:        (*requirement).to_owned(),
                alias:              None,
            }
        })
        .collect();
    store.insert_release(&release)?;
    Ok(())
}

fn pragma<T: rusqlite::types::FromSql>(database: &Connection, name: &str) -> rusqlite::Result<T> {
    database.pragma_query_value(None, name, |row| row.get(0))
}

fn versions(packages: &[den_package_store::ResolvedPackage]) -> Vec<(&str, &str)> {
    packages
        .iter()
        .map(|package| (package.package.as_str(), package.version.as_str()))
        .collect()
}

#[test]
fn rejects_missing_table_before_stamping_version() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("missing-table.sqlite3");
    drop(PackageStore::create(&path)?);
    let database = Connection::open(&path)?;
    database.execute_batch("DROP TABLE package_file")?;
    database.execute_batch("PRAGMA user_version = 0")?;
    drop(database);

    assert!(matches!(
        PackageStore::open(&path),
        Err(PackageStoreError::SchemaMismatch { object, .. }) if object == "package_file"
    ));
    assert_eq!(pragma::<i64>(&Connection::open(&path)?, "user_version")?, 0);
    Ok(())
}

#[test]
fn rejects_altered_required_column() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("altered-column.sqlite3");
    drop(PackageStore::create(&path)?);
    Connection::open(&path)?
        .execute_batch("ALTER TABLE registry RENAME COLUMN kind TO forged_kind")?;

    assert!(matches!(
        PackageStore::open(&path),
        Err(PackageStoreError::SchemaMismatch { object, .. })
            if object == "registry table definition"
    ));
    Ok(())
}

#[test]
fn rejects_altered_constraints_and_strictness() -> TestResult {
    assert_schema_rewrite_rejected(
        "blob",
        "CHECK (length(\"digest\") = 32)",
        "CHECK (length(\"digest\") >= 0)",
    )?;
    assert_schema_rewrite_rejected(
        "package",
        "UNIQUE (\"registry_id\", \"name\")",
        "UNIQUE (\"registry_id\", \"registry_id\")",
    )?;
    assert_schema_rewrite_rejected(
        "package_version",
        "ON DELETE CASCADE",
        "ON DELETE NO ACTION",
    )?;
    assert_schema_rewrite_rejected("export", ") STRICT", ")")
}

fn assert_schema_rewrite_rejected(table: &str, from: &str, to: &str) -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join(format!("altered-{table}.sqlite3"));
    drop(PackageStore::create(&path)?);
    let database = Connection::open(&path)?;
    let sql: String = database.query_row(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get(0),
    )?;
    let altered = sql.replacen(from, to, 1);
    if altered == sql {
        return Err(format!("fixture table `{table}` does not contain `{from}`").into());
    }
    let schema_version: i64 = pragma(&database, "schema_version")?;
    let next_schema_version = schema_version
        .checked_add(1)
        .ok_or("fixture schema version overflowed")?;
    database.execute_batch("PRAGMA writable_schema = ON")?;
    database.execute(
        "UPDATE sqlite_schema SET sql = ?1 WHERE type = 'table' AND name = ?2",
        (altered, table),
    )?;
    database.execute_batch(&format!("PRAGMA schema_version = {next_schema_version}"))?;
    database.execute_batch("PRAGMA writable_schema = OFF")?;
    drop(database);

    match PackageStore::open(&path) {
        Err(PackageStoreError::SchemaMismatch { object, .. })
            if object == format!("{table} table definition") =>
        {
            Ok(())
        }
        result => {
            Err(format!("unexpected open result after altering `{table}`: {result:?}").into())
        }
    }
}
