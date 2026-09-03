use den_package_store::{
    CancellationToken, DependencyKind, NewDependency, NewExport, NewPackageFile, NewRelease,
    PackageStore, PackageStoreError, RegistryId, RootRequirement,
};
use sea_orm::{ConnectionTrait as _, Database, DbBackend, Statement};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn creates_migrates_and_reopens_store() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("packages.sqlite3");
    let store = PackageStore::create(&path).await?;
    let registry_id = store
        .add_registry("npm", "https://registry.example/")
        .await?;
    drop(store);

    let reopened = PackageStore::open(&path).await?;
    assert_eq!(
        reopened
            .registry_id("npm", "https://registry.example/")
            .await?,
        Some(registry_id)
    );
    drop(reopened);

    let database = Database::connect(format!("sqlite://{}?mode=rw", path.display())).await?;
    assert_eq!(pragma(&database, "application_id").await?, 0x4445_4e50);
    assert_eq!(pragma(&database, "user_version").await?, 1);
    assert_eq!(pragma_text(&database, "journal_mode").await?, "wal");
    let non_strict = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) FROM pragma_table_list WHERE schema = 'main' AND name NOT LIKE \
             'sqlite_%' AND strict = 0"
                .to_owned(),
        ))
        .await?
        .ok_or("strict-table query returned no row")?
        .try_get_by_index::<i64>(0)?;
    assert_eq!(non_strict, 0);
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn relative_store_paths_create_and_reopen() -> TestResult {
    let directory = tempfile::tempdir_in(".")?;
    let current = std::env::current_dir()?;
    let path = directory
        .path()
        .join("relative.sqlite3")
        .strip_prefix(&current)?
        .to_path_buf();
    assert!(path.is_relative());
    let store = PackageStore::create(&path).await?;
    let registry = store.add_registry("jsr", "https://jsr.example/").await?;
    drop(store);
    assert_eq!(
        PackageStore::open(&path)
            .await?
            .registry_id("jsr", "https://jsr.example/")
            .await?,
        Some(registry)
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn non_utf8_store_paths_fail_with_an_explicit_driver_limit() -> TestResult {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt as _};

    let directory = tempfile::tempdir()?;
    let path = directory
        .path()
        .join(OsString::from_vec(b"packages-\xff.sqlite3".to_vec()));
    assert!(matches!(
        PackageStore::create(&path).await,
        Err(PackageStoreError::InvalidDatabasePath(_))
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_newer_schema() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("future.sqlite3");
    drop(PackageStore::create(&path).await?);
    let database = Database::connect(format!("sqlite://{}?mode=rw", path.display())).await?;
    database
        .execute_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA user_version = 2".to_owned(),
        ))
        .await?;
    database.close().await?;

    assert!(matches!(
        PackageStore::open(&path).await,
        Err(PackageStoreError::SchemaTooNew {
            actual:    2,
            supported: 1,
        })
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_unknown_seaorm_migration() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("unknown-migration.sqlite3");
    drop(PackageStore::create(&path).await?);
    let database = Database::connect(format!("sqlite://{}?mode=rw", path.display())).await?;
    database
        .execute_raw(Statement::from_string(
            DbBackend::Sqlite,
            "INSERT INTO den_package_store_migrations(version, applied_at) VALUES \
             ('m99999999_999999_future', 0)"
                .to_owned(),
        ))
        .await?;
    database.close().await?;

    assert!(matches!(
        PackageStore::open(&path).await,
        Err(PackageStoreError::UnknownMigration(version))
            if version == "m99999999_999999_future"
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_known_migration_with_missing_table_before_stamping_version() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("missing-table.sqlite3");
    drop(PackageStore::create(&path).await?);
    let database = Database::connect(format!("sqlite://{}?mode=rw", path.display())).await?;
    database
        .execute_raw(Statement::from_string(
            DbBackend::Sqlite,
            "DROP TABLE package_file".to_owned(),
        ))
        .await?;
    database
        .execute_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA user_version = 0".to_owned(),
        ))
        .await?;
    database.close().await?;

    assert!(matches!(
        PackageStore::open(&path).await,
        Err(PackageStoreError::SchemaMismatch { object, .. }) if object == "package_file"
    ));
    let database = Database::connect(format!("sqlite://{}?mode=rw", path.display())).await?;
    assert_eq!(pragma(&database, "user_version").await?, 0);
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn rejects_altered_required_column() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("altered-column.sqlite3");
    drop(PackageStore::create(&path).await?);
    let database = Database::connect(format!("sqlite://{}?mode=rw", path.display())).await?;
    database
        .execute_raw(Statement::from_string(
            DbBackend::Sqlite,
            "ALTER TABLE registry RENAME COLUMN kind TO forged_kind".to_owned(),
        ))
        .await?;
    database.close().await?;

    assert!(matches!(
        PackageStore::open(&path).await,
        Err(PackageStoreError::SchemaMismatch { object, .. })
            if object == "registry table definition"
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_altered_constraints_and_strictness() -> TestResult {
    assert_schema_rewrite_rejected(
        "blob",
        "CHECK (length(\"digest\") = 32)",
        "CHECK (length(\"digest\") >= 0)",
    )
    .await?;
    assert_schema_rewrite_rejected(
        "package",
        "UNIQUE (\"registry_id\", \"name\")",
        "UNIQUE (\"registry_id\", \"registry_id\")",
    )
    .await?;
    assert_schema_rewrite_rejected(
        "package_version",
        "ON DELETE CASCADE",
        "ON DELETE NO ACTION",
    )
    .await?;
    assert_schema_rewrite_rejected("export", ") STRICT", ")").await
}

#[tokio::test]
async fn rejects_and_preserves_foreign_application_identity() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("foreign.sqlite3");
    let database = Database::connect(format!("sqlite://{}?mode=rwc", path.display())).await?;
    database
        .execute_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA application_id = 305419896".to_owned(),
        ))
        .await?;
    database.close().await?;

    assert!(matches!(
        PackageStore::open(&path).await,
        Err(PackageStoreError::ForeignDatabase {
            actual: 305_419_896,
        })
    ));
    let database = Database::connect(format!("sqlite://{}?mode=rw", path.display())).await?;
    assert_eq!(pragma(&database, "application_id").await?, 305_419_896);
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn blobs_deduplicate() -> TestResult {
    let store = PackageStore::open_in_memory().await?;
    let digest = store.insert_blob(b"original").await?;
    assert_eq!(store.insert_blob(b"original").await?, digest);
    Ok(())
}

#[tokio::test]
async fn registries_are_canonical_and_reject_embedded_credentials() -> TestResult {
    let store = PackageStore::open_in_memory().await?;
    let first = store
        .add_registry("jsr", "https://registry.example/api")
        .await?;
    let second = store
        .add_registry("jsr", "https://registry.example/api/")
        .await?;
    assert_eq!(first, second);
    assert!(matches!(
        store
            .add_registry("jsr", "https://token@registry.example/")
            .await,
        Err(PackageStoreError::InvalidRegistry(_))
    ));
    Ok(())
}

#[tokio::test]
async fn registry_lookup_does_not_create_missing_rows() -> TestResult {
    let store = PackageStore::open_in_memory().await?;
    assert_eq!(
        store.registry_id("jsr", "https://jsr.example/").await?,
        None
    );
    let id = store.add_registry("jsr", "https://jsr.example/").await?;
    assert_eq!(
        store.registry_id("jsr", "https://jsr.example").await?,
        Some(id)
    );
    assert!(
        store
            .registry_id("npm", "https://jsr.example/")
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn concurrent_stores_insert_distinct_versions_of_one_new_package() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("concurrent.sqlite3");
    let first = PackageStore::create(&path).await?;
    let registry = first.add_registry("jsr", "https://jsr.example/").await?;
    let second = PackageStore::open(&path).await?;
    let one = NewRelease::new(registry, "shared", "1.0.0");
    let two = NewRelease::new(registry, "shared", "2.0.0");

    let (one, two) = tokio::join!(first.insert_release(&one), second.insert_release(&two));
    one?;
    two?;
    let snapshot = first.repository_snapshot().await?;
    for version in ["1.0.0", "2.0.0"] {
        let solved = snapshot.solve(&[RootRequirement::new(registry, "shared", version)])?;
        assert_eq!(versions(&solved.packages), vec![("shared", version)]);
    }
    Ok(())
}

#[tokio::test]
async fn invalid_release_leaves_no_partial_package() -> TestResult {
    let store = PackageStore::open_in_memory().await?;
    let registry_id = store.add_registry("jsr", "https://jsr.example/").await?;
    let digest = store.insert_blob(b"export default 1").await?;
    let mut release = NewRelease::new(registry_id, "@scope/pkg", "1.0.0");
    release.files.push(NewPackageFile {
        path:       "../escape.ts".to_owned(),
        blob:       digest,
        media_type: Some("text/typescript".to_owned()),
        mode:       0o644,
    });

    assert!(matches!(
        store.insert_release(&release).await,
        Err(PackageStoreError::InvalidModulePath { .. })
    ));
    assert!(
        store
            .repository_snapshot()
            .await?
            .solve(&[RootRequirement::new(registry_id, "@scope/pkg", "*")])
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn dangling_export_is_rejected_before_commit() -> TestResult {
    let store = PackageStore::open_in_memory().await?;
    let registry_id = store.add_registry("jsr", "https://jsr.example/").await?;
    let mut release = NewRelease::new(registry_id, "@scope/pkg", "1.0.0");
    release.exports.push(NewExport {
        name:   ".".to_owned(),
        target: "missing.ts".to_owned(),
    });

    assert!(matches!(
        store.insert_release(&release).await,
        Err(PackageStoreError::MissingExportTarget { .. })
    ));
    assert!(
        store
            .repository_snapshot()
            .await?
            .solve(&[RootRequirement::new(registry_id, "@scope/pkg", "*")])
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn solver_selects_highest_compatible_transitive_version() -> TestResult {
    let (store, registry_id) = store_with_registry().await?;
    insert_release(
        &store,
        registry_id,
        "app",
        "1.0.0",
        &[("dep", "^1.0.0")],
        None,
    )
    .await?;
    insert_release(&store, registry_id, "dep", "1.0.0", &[], None).await?;
    insert_release(&store, registry_id, "dep", "1.8.0", &[], None).await?;
    insert_release(&store, registry_id, "dep", "2.0.0", &[], None).await?;

    let solved = store
        .repository_snapshot()
        .await?
        .solve(&[RootRequirement::new(registry_id, "app", "*")])?;
    assert_eq!(versions(&solved.packages), vec![
        ("app", "1.0.0"),
        ("dep", "1.8.0")
    ]);
    Ok(())
}

#[tokio::test]
async fn solver_reports_conflicts_and_excludes_yanked_versions() -> TestResult {
    let (store, registry_id) = store_with_registry().await?;
    insert_release(
        &store,
        registry_id,
        "left",
        "1.0.0",
        &[("dep", "^1.0.0")],
        None,
    )
    .await?;
    insert_release(
        &store,
        registry_id,
        "right",
        "1.0.0",
        &[("dep", "^2.0.0")],
        None,
    )
    .await?;
    insert_release(
        &store,
        registry_id,
        "dep",
        "1.9.0",
        &[],
        Some("bad archive"),
    )
    .await?;
    insert_release(&store, registry_id, "dep", "1.8.0", &[], None).await?;
    insert_release(&store, registry_id, "dep", "2.0.0", &[], None).await?;

    let snapshot = store.repository_snapshot().await?;
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

#[tokio::test]
async fn repeated_solves_are_deterministic() -> TestResult {
    let (store, registry_id) = store_with_registry().await?;
    insert_release(&store, registry_id, "app", "1.0.0", &[("dep", "*")], None).await?;
    insert_release(&store, registry_id, "app", "1.1.0", &[("dep", "^1")], None).await?;
    insert_release(&store, registry_id, "dep", "1.0.0", &[], None).await?;
    insert_release(&store, registry_id, "dep", "1.1.0", &[], None).await?;
    let snapshot = store.repository_snapshot().await?;
    let roots = [RootRequirement::new(registry_id, "app", "*")];
    let expected = snapshot.solve(&roots)?;
    for _ in 0..20 {
        assert_eq!(snapshot.solve(&roots)?, expected);
    }
    Ok(())
}

#[tokio::test]
async fn solve_can_be_cancelled_before_start() -> TestResult {
    let (store, registry_id) = store_with_registry().await?;
    insert_release(&store, registry_id, "app", "1.0.0", &[], None).await?;
    let snapshot = store.repository_snapshot().await?;
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    assert!(matches!(
        snapshot.solve_with_cancellation(
            &[RootRequirement::new(registry_id, "app", "*")],
            Some(cancellation)
        ),
        Err(PackageStoreError::Cancelled)
    ));
    Ok(())
}

#[tokio::test]
async fn flat_solver_rejects_optional_or_peer_semantics_instead_of_lying() -> TestResult {
    let (store, registry_id) = store_with_registry().await?;
    let mut release = NewRelease::new(registry_id, "app", "1.0.0");
    release.dependencies.push(NewDependency {
        kind:               DependencyKind::Optional,
        target_registry_id: None,
        package:            "optional-dep".to_owned(),
        requirement:        "^1".to_owned(),
        alias:              None,
    });
    store.insert_release(&release).await?;

    let error = store
        .repository_snapshot()
        .await?
        .solve(&[RootRequirement::new(registry_id, "app", "*")])
        .expect_err("flat solver must exclude optional semantics");
    assert!(matches!(error, PackageStoreError::Conflict(_)));
    assert!(error.to_string().contains("optional"));
    Ok(())
}

#[tokio::test]
async fn flat_solver_excludes_dependency_aliases_instead_of_dropping_them() -> TestResult {
    let (store, registry_id) = store_with_registry().await?;
    let mut release = NewRelease::new(registry_id, "app", "1.0.0");
    release.dependencies.push(NewDependency {
        kind:               DependencyKind::Normal,
        target_registry_id: None,
        package:            "real-name".to_owned(),
        requirement:        "^1".to_owned(),
        alias:              Some("alias-name".to_owned()),
    });
    store.insert_release(&release).await?;

    let error = store
        .repository_snapshot()
        .await?
        .solve(&[RootRequirement::new(registry_id, "app", "*")])
        .expect_err("flat solver must exclude aliases");
    assert!(error.to_string().contains("alias-name"));
    Ok(())
}

async fn store_with_registry() -> Result<(PackageStore, RegistryId), Box<dyn std::error::Error>> {
    let store = PackageStore::open_in_memory().await?;
    let registry_id = store
        .add_registry("npm", "https://registry.example/")
        .await?;
    Ok((store, registry_id))
}

async fn insert_release(
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
    store.insert_release(&release).await?;
    Ok(())
}

async fn assert_schema_rewrite_rejected(table: &str, from: &str, to: &str) -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join(format!("altered-{table}.sqlite3"));
    drop(PackageStore::create(&path).await?);
    let database = Database::connect(format!("sqlite://{}?mode=rw", path.display())).await?;
    let sql = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?",
            [table.to_owned().into()],
        ))
        .await?
        .ok_or("fixture table has no sqlite_schema row")?
        .try_get::<String>("", "sql")?;
    let altered = sql.replacen(from, to, 1);
    if altered == sql {
        return Err(format!("fixture table `{table}` does not contain `{from}`").into());
    }
    database
        .execute_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA writable_schema = ON".to_owned(),
        ))
        .await?;
    database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE sqlite_schema SET sql = ? WHERE type = 'table' AND name = ?",
            [altered.into(), table.to_owned().into()],
        ))
        .await?;
    let schema_version = pragma(&database, "schema_version").await?;
    let next_schema_version = schema_version
        .checked_add(1)
        .ok_or("fixture schema version overflowed")?;
    database
        .execute_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("PRAGMA schema_version = {next_schema_version}"),
        ))
        .await?;
    database
        .execute_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA writable_schema = OFF".to_owned(),
        ))
        .await?;
    database.close().await?;

    match PackageStore::open(&path).await {
        Err(PackageStoreError::SchemaMismatch { object, .. })
            if object == format!("{table} table definition") => {}
        result => {
            return Err(
                format!("unexpected open result after altering `{table}`: {result:?}").into(),
            );
        }
    }
    Ok(())
}

async fn pragma(database: &sea_orm::DatabaseConnection, name: &str) -> Result<i64, sea_orm::DbErr> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("PRAGMA {name}"),
        ))
        .await?
        .ok_or_else(|| sea_orm::DbErr::Custom(format!("PRAGMA {name} returned no row")))?;
    row.try_get_by_index(0)
}

async fn pragma_text(
    database: &sea_orm::DatabaseConnection, name: &str,
) -> Result<String, sea_orm::DbErr> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("PRAGMA {name}"),
        ))
        .await?
        .ok_or_else(|| sea_orm::DbErr::Custom(format!("PRAGMA {name} returned no row")))?;
    row.try_get_by_index(0)
}

fn versions(packages: &[den_package_store::ResolvedPackage]) -> Vec<(&str, &str)> {
    packages
        .iter()
        .map(|package| (package.package.as_str(), package.version.as_str()))
        .collect()
}
