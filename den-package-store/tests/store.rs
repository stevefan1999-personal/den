use den_package_store::{
    CancellationToken, DependencyKind, NewDependency, NewExport, NewPackageFile, NewRelease,
    PackageStore, PackageStoreError, RegistryId, RootRequirement,
};
use sea_orm::{ConnectionTrait as _, Database, DbBackend, Statement};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions preserve useful diffs"
)]
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
            .registry(registry_id)
            .await?
            .map(|registry| registry.kind),
        Some("npm".to_owned())
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion verifies native relative-path handling"
)]
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
    assert!(
        PackageStore::open(&path)
            .await?
            .registry(registry)
            .await?
            .is_some()
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion documents the SQLite driver's UTF-8 path limit"
)]
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion verifies error shape"
)]
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion verifies migration refusal"
)]
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion verifies forged migration state is rejected before stamping"
)]
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion verifies altered required columns are rejected"
)]
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
        Err(PackageStoreError::SchemaMismatch { object, .. }) if object == "registry"
    ));
    Ok(())
}

#[tokio::test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions preserve useful diffs"
)]
async fn blobs_deduplicate() -> TestResult {
    let store = PackageStore::open_in_memory().await?;
    let digest = store.insert_blob(b"original").await?;
    assert_eq!(store.insert_blob(b"original").await?, digest);
    assert_eq!(store.read_blob(digest).await?, b"original");
    Ok(())
}

#[tokio::test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions verify registry canonicalization and validation"
)]
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion verifies concurrent upsert convergence"
)]
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
    assert_eq!(
        first
            .package(registry, "shared")
            .await?
            .ok_or("package missing")?
            .versions
            .len(),
        2
    );
    Ok(())
}

#[tokio::test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions verify atomic rejection"
)]
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
    assert!(store.package(registry_id, "@scope/pkg").await?.is_none());
    Ok(())
}

#[tokio::test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions verify dangling exports fail atomically"
)]
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
    assert!(store.package(registry_id, "@scope/pkg").await?.is_none());
    Ok(())
}

#[tokio::test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions preserve useful diffs"
)]
async fn module_round_trips_from_sqlite_cas() -> TestResult {
    let store = PackageStore::open_in_memory().await?;
    let registry_id = store.add_registry("jsr", "https://jsr.example/").await?;
    let source = b"export const answer: number = 42;";
    let digest = store.insert_blob(source).await?;
    let mut release = NewRelease::new(registry_id, "@scope/pkg", "1.0.0");
    release.exports.push(NewExport {
        name:   ".".to_owned(),
        target: "src/mod.ts".to_owned(),
    });
    release.files.push(NewPackageFile {
        path:       "src/mod.ts".to_owned(),
        blob:       digest,
        media_type: Some("text/typescript".to_owned()),
        mode:       0o644,
    });
    store.insert_release(&release).await?;

    let module = store
        .module(registry_id, "@scope/pkg", "1.0.0", "src/mod.ts")
        .await?
        .ok_or("module was not returned")?;
    assert_eq!(module.digest, digest);
    assert_eq!(module.bytes, source);
    assert_eq!(module.media_type.as_deref(), Some("text/typescript"));
    Ok(())
}

#[tokio::test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions verify reachability-based content collection"
)]
async fn prune_removes_only_unreferenced_blobs() -> TestResult {
    let store = PackageStore::open_in_memory().await?;
    let registry_id = store.add_registry("jsr", "https://jsr.example/").await?;
    let orphan = store.insert_blob(b"orphan").await?;
    let live = store.insert_blob(b"live").await?;
    let mut release = NewRelease::new(registry_id, "live-package", "1.0.0");
    release.files.push(NewPackageFile {
        path:       "mod.js".to_owned(),
        blob:       live,
        media_type: Some("text/javascript".to_owned()),
        mode:       0o644,
    });
    store.insert_release(&release).await?;

    assert!(store.prune_unreferenced_blobs().await? >= 1);
    assert!(matches!(
        store.read_blob(orphan).await,
        Err(PackageStoreError::BlobNotFound(digest)) if digest == orphan
    ));
    assert_eq!(store.read_blob(live).await?, b"live");
    Ok(())
}

#[tokio::test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion preserves solver diff"
)]
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions verify diagnostics"
)]
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion preserves deterministic diff"
)]
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion verifies cancellation shape"
)]
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertion verifies unsupported semantics fail explicitly"
)]
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions verify aliases wait for persisted edge identities"
)]
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
