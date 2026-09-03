use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use node_semver::Version as SemverVersion;
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, TransactionBehavior};

use crate::{
    BlobDigest, DependencyKind, NewRelease, PackageKey, PackageStoreError, RegistryId,
    RepositorySnapshot, Result, SnapshotDependency, SnapshotVersion, VersionId, validation,
};

const APPLICATION_ID: i64 = 0x4445_4e50;
const SCHEMA_VERSION: i64 = 1;
/// Name the original SeaORM migrator logged for schema 1; stores created by it
/// must keep opening, so the log table and this row are preserved verbatim.
const MIGRATION_NAME: &str = "migration";
const MIGRATION_LOG: &str = "CREATE TABLE IF NOT EXISTS \"den_package_store_migrations\" ( \
                             \"version\" text NOT NULL PRIMARY KEY, \"applied_at\" integer NOT \
                             NULL ) STRICT";
/// Every table exactly as SQLite stores its `CREATE TABLE` text, so one
/// normalized compare in `validate_schema` covers columns, types,
/// constraints and STRICT for the whole schema.
const TABLES: [(&str, &str); 8] = [
    ("den_package_store_migrations", MIGRATION_LOG),
    (
        "blob",
        "CREATE TABLE \"blob\" ( \"digest\" blob NOT NULL PRIMARY KEY, \"bytes\" blob NOT NULL, \
         CHECK (length(\"digest\") = 32) ) STRICT",
    ),
    (
        "registry",
        "CREATE TABLE \"registry\" ( \"id\" integer NOT NULL PRIMARY KEY AUTOINCREMENT, \"kind\" \
         text NOT NULL, \"base_url\" text NOT NULL UNIQUE ) STRICT",
    ),
    (
        "package",
        "CREATE TABLE \"package\" ( \"id\" integer NOT NULL PRIMARY KEY AUTOINCREMENT, \
         \"registry_id\" integer NOT NULL, \"name\" text NOT NULL, CONSTRAINT \
         \"uq-package-registry-name\" UNIQUE (\"registry_id\", \"name\"), FOREIGN KEY \
         (\"registry_id\") REFERENCES \"registry\" (\"id\") ) STRICT",
    ),
    (
        "package_version",
        "CREATE TABLE \"package_version\" ( \"id\" integer NOT NULL PRIMARY KEY AUTOINCREMENT, \
         \"package_id\" integer NOT NULL, \"version\" text NOT NULL, \"published_at\" integer \
         NULL, \"yanked_reason\" text NULL, \"manifest_digest\" blob NOT NULL, CONSTRAINT \
         \"uq-package-version\" UNIQUE (\"package_id\", \"version\"), FOREIGN KEY \
         (\"package_id\") REFERENCES \"package\" (\"id\") ON DELETE CASCADE, FOREIGN KEY \
         (\"manifest_digest\") REFERENCES \"blob\" (\"digest\") ) STRICT",
    ),
    (
        "dependency",
        "CREATE TABLE \"dependency\" ( \"version_id\" integer NOT NULL, \"ordinal\" integer NOT \
         NULL, \"kind\" text NOT NULL, \"target_registry_id\" integer NULL, \"package_name\" text \
         NOT NULL, \"requirement\" text NOT NULL, \"alias\" text NULL, PRIMARY KEY \
         (\"version_id\", \"ordinal\"), FOREIGN KEY (\"version_id\") REFERENCES \
         \"package_version\" (\"id\") ON DELETE CASCADE, FOREIGN KEY (\"target_registry_id\") \
         REFERENCES \"registry\" (\"id\") ) STRICT",
    ),
    (
        "export",
        "CREATE TABLE \"export\" ( \"version_id\" integer NOT NULL, \"name\" text NOT NULL, \
         \"target_path\" text NOT NULL, PRIMARY KEY (\"version_id\", \"name\"), FOREIGN KEY \
         (\"version_id\") REFERENCES \"package_version\" (\"id\") ON DELETE CASCADE ) STRICT",
    ),
    (
        "package_file",
        "CREATE TABLE \"package_file\" ( \"version_id\" integer NOT NULL, \"path\" text NOT NULL, \
         \"blob_digest\" blob NOT NULL, \"media_type\" text NULL, \"mode\" integer NOT NULL, \
         PRIMARY KEY (\"version_id\", \"path\"), FOREIGN KEY (\"version_id\") REFERENCES \
         \"package_version\" (\"id\") ON DELETE CASCADE, FOREIGN KEY (\"blob_digest\") REFERENCES \
         \"blob\" (\"digest\") ) STRICT",
    ),
];

#[derive(Debug)]
pub struct PackageStore {
    connection: Mutex<Connection>,
}

impl PackageStore {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let flags = OpenFlags::default().difference(OpenFlags::SQLITE_OPEN_CREATE);
        Self::connect(Connection::open_with_flags(path, flags)?, false)
    }

    pub fn create<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::connect(Connection::open(path)?, false)
    }

    pub fn open_in_memory() -> Result<Self> { Self::connect(Connection::open_in_memory()?, true) }

    fn connect(connection: Connection, in_memory: bool) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "trusted_schema", "OFF")?;
        initialize(&connection, in_memory)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// One connection behind one lock serializes all store work. A poisoned
    /// lock only means another thread panicked mid-call; SQLite rolled its
    /// transaction back, so the connection is still consistent.
    pub(crate) fn lock(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn add_registry(&self, kind: &str, base_url: &str) -> Result<RegistryId> {
        let base_url = validate_registry(kind, base_url)?;
        add_registry_on(&self.lock(), kind, &base_url)
    }

    /// Look up a configured registry without mutating the store.
    pub fn registry_id(&self, kind: &str, base_url: &str) -> Result<Option<RegistryId>> {
        let base_url = validate_registry(kind, base_url)?;
        find_registry(&self.lock(), kind, &base_url)
    }

    pub fn insert_blob(&self, bytes: &[u8]) -> Result<BlobDigest> {
        insert_blob_on(&self.lock(), bytes)
    }

    pub fn insert_release(&self, release: &NewRelease) -> Result<VersionId> {
        validation::release(release)?;
        insert_release_on(&mut self.lock(), release)
    }

    pub fn repository_snapshot(&self) -> Result<RepositorySnapshot> {
        repository_snapshot_on(&mut self.lock())
    }
}

fn initialize(connection: &Connection, in_memory: bool) -> Result<()> {
    let application_id = pragma_i64(connection, "application_id")?;
    let user_version = pragma_i64(connection, "user_version")?;
    if user_version > SCHEMA_VERSION {
        return Err(PackageStoreError::SchemaTooNew {
            actual:    user_version,
            supported: SCHEMA_VERSION,
        });
    }
    if application_id != 0 && application_id != APPLICATION_ID {
        return Err(PackageStoreError::ForeignDatabase {
            actual: application_id,
        });
    }
    if application_id == 0 && !database_is_empty(connection)? {
        return Err(PackageStoreError::UnrecognizedDatabase);
    }

    connection.execute_batch(MIGRATION_LOG)?;
    if let Some(unknown) = connection
        .query_row(
            "SELECT version FROM den_package_store_migrations WHERE version <> ?1 LIMIT 1",
            [MIGRATION_NAME],
            |row| row.get(0),
        )
        .optional()?
    {
        return Err(PackageStoreError::UnknownMigration(unknown));
    }
    if !connection
        .prepare("SELECT 1 FROM den_package_store_migrations WHERE version = ?1")?
        .exists([MIGRATION_NAME])?
    {
        create_schema(connection)?;
    }
    validate_schema(connection)?;
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    if !in_memory {
        connection.pragma_update(None, "journal_mode", "WAL")?;
    }
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

fn create_schema(connection: &Connection) -> Result<()> {
    let mut batch = String::from("BEGIN;");
    for (_name, definition) in TABLES {
        batch.push_str(definition);
        batch.push(';');
    }
    batch.push_str("INSERT INTO den_package_store_migrations(version, applied_at) VALUES ('");
    batch.push_str(MIGRATION_NAME);
    batch.push_str("', unixepoch()); COMMIT;");
    Ok(connection.execute_batch(&batch)?)
}

/// Refuse a store whose tables were rewritten under us. SQLite keeps the
/// `CREATE TABLE` text verbatim, so comparing it against the definition this
/// build ships covers columns, types, defaults, constraints and STRICT at once.
fn validate_schema(connection: &Connection) -> Result<()> {
    for (name, expected) in TABLES {
        let actual: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| schema_mismatch(name, "table definition", "missing"))?;
        if normalized_table_sql(&actual) != normalized_table_sql(expected) {
            return Err(schema_mismatch(
                format!("{name} table definition"),
                normalized_table_sql(expected),
                normalized_table_sql(&actual),
            ));
        }
    }
    Ok(())
}

fn normalized_table_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("IF NOT EXISTS ", "")
}

fn schema_mismatch(
    object: impl Into<String>, expected: impl Into<String>, actual: impl Into<String>,
) -> PackageStoreError {
    PackageStoreError::SchemaMismatch {
        object:   object.into(),
        expected: expected.into(),
        actual:   actual.into(),
    }
}

fn pragma_i64(connection: &Connection, name: &str) -> Result<i64> {
    Ok(connection.pragma_query_value(None, name, |row| row.get(0))?)
}

fn database_is_empty(connection: &Connection) -> Result<bool> {
    let tables: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    Ok(tables == 0)
}

fn add_registry_on(connection: &Connection, kind: &str, base_url: &str) -> Result<RegistryId> {
    connection.execute(
        "INSERT OR IGNORE INTO registry(kind, base_url) VALUES (?1, ?2)",
        (kind, base_url),
    )?;
    find_registry(connection, kind, base_url)?.ok_or_else(|| {
        PackageStoreError::InvalidSnapshot(
            "registry upsert did not produce a readable row".to_owned(),
        )
    })
}

fn find_registry(
    connection: &Connection, kind: &str, base_url: &str,
) -> Result<Option<RegistryId>> {
    connection
        .query_row(
            "SELECT id, kind FROM registry WHERE base_url = ?1",
            [base_url],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .map(|(id, stored_kind)| {
            if stored_kind == kind {
                Ok(RegistryId(id))
            } else {
                Err(PackageStoreError::InvalidRegistry(format!(
                    "{base_url} is already registered as `{stored_kind}`, not `{kind}`"
                )))
            }
        })
        .transpose()
}

fn ensure_registry(connection: &Connection, id: RegistryId) -> Result<()> {
    if connection
        .prepare_cached("SELECT 1 FROM registry WHERE id = ?1")?
        .exists([id.0])?
    {
        Ok(())
    } else {
        Err(PackageStoreError::RegistryNotFound(id))
    }
}

fn insert_blob_on(connection: &Connection, bytes: &[u8]) -> Result<BlobDigest> {
    let digest = BlobDigest::for_bytes(bytes);
    connection.execute(
        "INSERT OR IGNORE INTO blob(digest, bytes) VALUES (?1, ?2)",
        (digest.as_bytes(), bytes),
    )?;
    // Re-read so a pre-existing row with tampered content is caught by its
    // hash.
    let stored: Vec<u8> = connection.query_row(
        "SELECT bytes FROM blob WHERE digest = ?1",
        [digest.as_bytes()],
        |row| row.get(0),
    )?;
    let actual = BlobDigest::for_bytes(&stored);
    if actual == digest {
        Ok(digest)
    } else {
        Err(PackageStoreError::BlobCorrupt {
            expected: digest,
            actual,
        })
    }
}

fn insert_release_on(connection: &mut Connection, release: &NewRelease) -> Result<VersionId> {
    // IMMEDIATE takes the write lock up front so a concurrent writer waits on
    // busy_timeout instead of failing the later read-to-write upgrade.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_registry(&transaction, release.registry_id)?;
    for item in &release.dependencies {
        if let Some(registry_id) = item.target_registry_id {
            ensure_registry(&transaction, registry_id)?;
        }
    }
    for file in &release.files {
        if !transaction
            .prepare_cached("SELECT 1 FROM blob WHERE digest = ?1")?
            .exists([file.blob.as_bytes()])?
        {
            return Err(PackageStoreError::BlobNotFound(file.blob));
        }
    }

    transaction.execute(
        "INSERT OR IGNORE INTO package(registry_id, name) VALUES (?1, ?2)",
        (release.registry_id.0, &release.package),
    )?;
    let package_id: i64 = transaction.query_row(
        "SELECT id FROM package WHERE registry_id = ?1 AND name = ?2",
        (release.registry_id.0, &release.package),
        |row| row.get(0),
    )?;
    if transaction
        .prepare_cached("SELECT 1 FROM package_version WHERE package_id = ?1 AND version = ?2")?
        .exists((package_id, &release.version))?
    {
        return Err(PackageStoreError::ReleaseExists {
            package: release.package.clone(),
            version: release.version.clone(),
        });
    }

    let manifest_digest = insert_blob_on(&transaction, &release.manifest)?;
    transaction.execute(
        "INSERT INTO package_version(package_id, version, published_at, yanked_reason, \
         manifest_digest) VALUES (?1, ?2, ?3, ?4, ?5)",
        (
            package_id,
            &release.version,
            release.published_at,
            &release.yanked_reason,
            manifest_digest.as_bytes(),
        ),
    )?;
    let version_id = VersionId(transaction.last_insert_rowid());

    for (ordinal, item) in release.dependencies.iter().enumerate() {
        let ordinal = i64::try_from(ordinal).map_err(|_conversion_error| {
            PackageStoreError::InvalidSnapshot("too many dependencies in release".to_owned())
        })?;
        transaction.execute(
            "INSERT INTO dependency(version_id, ordinal, kind, target_registry_id, package_name, \
             requirement, alias) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                version_id.0,
                ordinal,
                item.kind.as_str(),
                item.target_registry_id.map(|id| id.0),
                &item.package,
                &item.requirement,
                &item.alias,
            ),
        )?;
    }
    for item in &release.exports {
        transaction.execute(
            "INSERT INTO export(version_id, name, target_path) VALUES (?1, ?2, ?3)",
            (version_id.0, &item.name, &item.target),
        )?;
    }
    for item in &release.files {
        transaction.execute(
            "INSERT INTO package_file(version_id, path, blob_digest, media_type, mode) VALUES \
             (?1, ?2, ?3, ?4, ?5)",
            (
                version_id.0,
                &item.path,
                item.blob.as_bytes(),
                &item.media_type,
                i64::from(item.mode),
            ),
        )?;
    }
    transaction.commit()?;
    Ok(version_id)
}

fn repository_snapshot_on(connection: &mut Connection) -> Result<RepositorySnapshot> {
    // One read transaction keeps the three scans on a single consistent
    // snapshot.
    let transaction = connection.transaction()?;
    let packages_by_id = transaction
        .prepare("SELECT id, registry_id, name FROM package ORDER BY id")?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .map(|row| {
            let (id, registry_id, name) = row?;
            validation::package_name(&name).map_err(|error| {
                PackageStoreError::InvalidSnapshot(format!(
                    "stored package name failed validation: {error}"
                ))
            })?;
            Ok((id, PackageKey {
                registry_id: RegistryId(registry_id),
                name,
            }))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let version_rows = transaction
        .prepare("SELECT id, package_id, version, yanked_reason FROM package_version ORDER BY id")?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let dependency_rows = transaction
        .prepare(
            "SELECT version_id, kind, target_registry_id, package_name, requirement, alias FROM \
             dependency ORDER BY version_id, ordinal",
        )?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(transaction);

    let package_by_version = version_rows
        .iter()
        .map(|(id, package_id, ..)| (*id, *package_id))
        .collect::<BTreeMap<_, _>>();
    let mut dependencies = BTreeMap::<VersionId, Vec<SnapshotDependency>>::new();
    for (version_id, kind, target_registry_id, package_name, requirement, alias) in dependency_rows
    {
        let source_package_id = package_by_version.get(&version_id).ok_or_else(|| {
            PackageStoreError::InvalidSnapshot("dependency refers to a missing version".to_owned())
        })?;
        let source_package = packages_by_id.get(source_package_id).ok_or_else(|| {
            PackageStoreError::InvalidSnapshot("version refers to a missing package".to_owned())
        })?;
        validation::package_name(&package_name).map_err(|error| {
            PackageStoreError::InvalidSnapshot(format!(
                "stored dependency name failed validation: {error}"
            ))
        })?;
        let parsed_requirement = node_semver::Range::parse(&requirement).map_err(|error| {
            PackageStoreError::InvalidSnapshot(format!(
                "stored dependency range `{requirement}` failed validation: {error}"
            ))
        })?;
        dependencies
            .entry(VersionId(version_id))
            .or_default()
            .push(SnapshotDependency {
                kind: DependencyKind::from_database(&kind)?,
                alias,
                package_key: PackageKey {
                    registry_id: target_registry_id.map_or(source_package.registry_id, RegistryId),
                    name:        package_name,
                },
                requirement,
                parsed_requirement,
            });
    }

    let mut snapshot = RepositorySnapshot::default();
    for (id, package_id, version, yanked_reason) in version_rows {
        let package_key = packages_by_id.get(&package_id).ok_or_else(|| {
            PackageStoreError::InvalidSnapshot("version refers to a missing package".to_owned())
        })?;
        let parsed_version = SemverVersion::parse(&version).map_err(|error| {
            PackageStoreError::InvalidSnapshot(format!(
                "stored version `{version}` failed validation: {error}"
            ))
        })?;
        snapshot
            .packages
            .entry(package_key.clone())
            .or_default()
            .push(SnapshotVersion {
                id: VersionId(id),
                raw_version: version,
                parsed_version,
                yanked_reason,
                dependencies: dependencies.remove(&VersionId(id)).unwrap_or_default(),
            });
    }
    if !dependencies.is_empty() {
        return Err(PackageStoreError::InvalidSnapshot(
            "dependencies refer to missing package versions".to_owned(),
        ));
    }
    Ok(snapshot)
}

fn validate_registry(kind: &str, base_url: &str) -> Result<String> {
    if kind.is_empty() || kind.contains('\0') {
        return Err(PackageStoreError::InvalidRegistry(format!(
            "registry kind `{kind}` must be non-empty"
        )));
    }
    let mut url = url::Url::parse(base_url)
        .map_err(|error| PackageStoreError::InvalidRegistry(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PackageStoreError::InvalidRegistry(
            "registry URL must be an HTTP(S) origin/path without credentials, query, or fragment"
                .to_owned(),
        ));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::PackageStore;
    use crate::{NewRelease, PackageStoreError};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn configures_connection_pragmas() -> TestResult {
        let store = PackageStore::open_in_memory()?;
        let connection = store.lock();
        assert_eq!(super::pragma_i64(&connection, "foreign_keys")?, 1);
        assert_eq!(super::pragma_i64(&connection, "trusted_schema")?, 0);
        assert_eq!(super::pragma_i64(&connection, "synchronous")?, 2);
        assert_eq!(super::pragma_i64(&connection, "busy_timeout")?, 5_000);
        Ok(())
    }

    #[test]
    fn detects_corrupt_blob_content() -> TestResult {
        let store = PackageStore::open_in_memory()?;
        let digest = store.insert_blob(b"original")?;
        store.lock().execute(
            "UPDATE blob SET bytes = ?1 WHERE digest = ?2",
            (b"tampered".as_slice(), digest.as_bytes()),
        )?;
        assert!(matches!(
            store.insert_blob(b"original"),
            Err(PackageStoreError::BlobCorrupt { expected, .. }) if expected == digest
        ));
        let kept: i64 = store.lock().query_row(
            "SELECT COUNT(*) FROM blob WHERE digest = ?1",
            [digest.as_bytes()],
            |row| row.get(0),
        )?;
        assert_eq!(kept, 1);
        Ok(())
    }

    #[test]
    fn release_transaction_rolls_back_package_on_mid_commit_failure() -> TestResult {
        let store = PackageStore::open_in_memory()?;
        let registry = store.add_registry("jsr", "https://jsr.example/")?;
        store.lock().execute_batch(
            "CREATE TRIGGER fail_package_version BEFORE INSERT ON package_version BEGIN SELECT \
             RAISE(ABORT, 'forced test failure'); END",
        )?;

        assert!(
            store
                .insert_release(&NewRelease::new(registry, "rollback", "1.0.0"))
                .is_err()
        );
        let packages: i64 = store
            .lock()
            .query_row("SELECT COUNT(*) FROM package", [], |row| row.get(0))?;
        assert_eq!(packages, 0);
        Ok(())
    }
}
