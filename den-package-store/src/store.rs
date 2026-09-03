use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};

use node_semver::Version as SemverVersion;
use sea_orm::{
    ActiveModelTrait as _,
    ActiveValue::Set,
    ColumnTrait as _, ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend,
    EntityTrait as _, QueryFilter as _, QueryOrder as _, Statement, TransactionTrait as _,
    sea_query::{OnConflict, TableCreateStatement},
    sqlx::sqlite::SqliteSynchronous,
};
use sea_orm_migration::{MigratorTrait as _, SchemaManager};

use crate::{
    BlobDigest, NewRelease, PackageKey, PackageStoreError, RegistryId, RepositorySnapshot, Result,
    SnapshotDependency, SnapshotVersion, VersionId,
    entity::{blob, dependency, package, package_export, package_file, package_version, registry},
    migration::{Migrator, expected_tables, install_tracking_table},
    validation,
};

const APPLICATION_ID: i64 = 0x4445_4e50;
const SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Debug)]
pub struct PackageStore {
    database: DatabaseConnection,
}

impl PackageStore {
    pub(crate) const fn database(&self) -> &DatabaseConnection { &self.database }

    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::connect_path(path.as_ref(), false).await
    }

    pub async fn create<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::connect_path(path.as_ref(), true).await
    }

    async fn connect_path(path: &Path, create: bool) -> Result<Self> {
        if path.to_str().is_none() {
            return Err(PackageStoreError::InvalidDatabasePath(
                "SeaORM's SQLite driver requires a UTF-8 path".to_owned(),
            ));
        }
        Self::connect_url(
            "sqlite://den-package-store-placeholder".to_owned(),
            false,
            Some((path.to_path_buf(), create)),
        )
        .await
    }

    pub async fn open_in_memory() -> Result<Self> {
        Self::connect_url("sqlite::memory:".to_owned(), true, None).await
    }

    async fn connect_url(
        database_url: String, in_memory: bool, file: Option<(std::path::PathBuf, bool)>,
    ) -> Result<Self> {
        let mut options = ConnectOptions::new(database_url);
        // ponytail: one pooled connection serializes store work; raise this
        // when concurrent package hydration proves it needs more read
        // throughput.
        options
            .max_connections(1)
            .min_connections(1)
            .sqlx_logging(false);
        options.map_sqlx_sqlite_opts(move |options| {
            let options = match &file {
                Some((path, create)) => {
                    options
                        .filename(path)
                        .in_memory(false)
                        .create_if_missing(*create)
                }
                None => options,
            };
            options
                .foreign_keys(true)
                .busy_timeout(Duration::from_secs(5))
                .synchronous(SqliteSynchronous::Full)
                .pragma("trusted_schema", "OFF")
        });
        let database = Database::connect(options).await?;
        initialize(&database, in_memory).await?;
        Ok(Self { database })
    }

    pub async fn add_registry(&self, kind: &str, base_url: &str) -> Result<RegistryId> {
        let base_url = validate_registry(kind, base_url)?;
        if let Some(existing) = registry::Entity::find()
            .filter(registry::Column::BaseUrl.eq(&base_url))
            .one(&self.database)
            .await?
        {
            return registry_identity(existing, kind);
        }

        let inserted = registry::ActiveModel {
            id:       sea_orm::ActiveValue::NotSet,
            kind:     Set(kind.to_owned()),
            base_url: Set(base_url.clone()),
        }
        .insert(&self.database)
        .await;
        match inserted {
            Ok(model) => Ok(RegistryId(model.id)),
            Err(insert_error) => {
                let existing = registry::Entity::find()
                    .filter(registry::Column::BaseUrl.eq(&base_url))
                    .one(&self.database)
                    .await?;
                existing.map_or_else(
                    || Err(insert_error.into()),
                    |existing| registry_identity(existing, kind),
                )
            }
        }
    }

    /// Look up a configured registry without mutating the store.
    pub async fn registry_id(&self, kind: &str, base_url: &str) -> Result<Option<RegistryId>> {
        let base_url = validate_registry(kind, base_url)?;
        registry::Entity::find()
            .filter(registry::Column::BaseUrl.eq(base_url))
            .one(&self.database)
            .await?
            .map(|model| registry_identity(model, kind))
            .transpose()
    }

    pub async fn insert_blob(&self, bytes: &[u8]) -> Result<BlobDigest> {
        insert_blob_on(&self.database, bytes).await
    }

    pub async fn insert_release(&self, release: &NewRelease) -> Result<VersionId> {
        validation::release(release)?;
        ensure_registry(&self.database, release.registry_id).await?;
        for item in &release.dependencies {
            if let Some(registry_id) = item.target_registry_id {
                ensure_registry(&self.database, registry_id).await?;
            }
        }
        for file in &release.files {
            if blob::Entity::find_by_id(file.blob.as_bytes().to_vec())
                .one(&self.database)
                .await?
                .is_none()
            {
                return Err(PackageStoreError::BlobNotFound(file.blob));
            }
        }

        let transaction = self.database.begin().await?;
        package::Entity::insert(package::ActiveModel {
            id:          sea_orm::ActiveValue::NotSet,
            registry_id: Set(release.registry_id.0),
            name:        Set(release.package.clone()),
        })
        .on_conflict(
            OnConflict::columns([package::Column::RegistryId, package::Column::Name])
                .do_nothing()
                .to_owned(),
        )
        .exec_without_returning(&transaction)
        .await?;
        let package_model = package::Entity::find()
            .filter(package::Column::RegistryId.eq(release.registry_id.0))
            .filter(package::Column::Name.eq(&release.package))
            .one(&transaction)
            .await?
            .ok_or_else(|| {
                PackageStoreError::InvalidSnapshot(
                    "package upsert did not produce a readable row".to_owned(),
                )
            })?;
        if package_version::Entity::find()
            .filter(package_version::Column::PackageId.eq(package_model.id))
            .filter(package_version::Column::Version.eq(&release.version))
            .one(&transaction)
            .await?
            .is_some()
        {
            return Err(PackageStoreError::ReleaseExists {
                package: release.package.clone(),
                version: release.version.clone(),
            });
        }

        let manifest_digest = insert_blob_on(&transaction, &release.manifest).await?;
        let version_model = package_version::ActiveModel {
            id:              sea_orm::ActiveValue::NotSet,
            package_id:      Set(package_model.id),
            version:         Set(release.version.clone()),
            published_at:    Set(release.published_at),
            yanked_reason:   Set(release.yanked_reason.clone()),
            manifest_digest: Set(manifest_digest.as_bytes().to_vec()),
        }
        .insert(&transaction)
        .await?;
        let version_id = VersionId(version_model.id);

        for (ordinal, item) in release.dependencies.iter().enumerate() {
            dependency::ActiveModel {
                version_id:         Set(version_id.0),
                ordinal:            Set(i64::try_from(ordinal).map_err(|_conversion_error| {
                    PackageStoreError::InvalidSnapshot(
                        "too many dependencies in release".to_owned(),
                    )
                })?),
                kind:               Set(item.kind.as_str().to_owned()),
                target_registry_id: Set(item.target_registry_id.map(|id| id.0)),
                package_name:       Set(item.package.clone()),
                requirement:        Set(item.requirement.clone()),
                alias:              Set(item.alias.clone()),
            }
            .insert(&transaction)
            .await?;
        }
        for item in &release.exports {
            package_export::ActiveModel {
                version_id:  Set(version_id.0),
                name:        Set(item.name.clone()),
                target_path: Set(item.target.clone()),
            }
            .insert(&transaction)
            .await?;
        }
        for item in &release.files {
            package_file::ActiveModel {
                version_id:  Set(version_id.0),
                path:        Set(item.path.clone()),
                blob_digest: Set(item.blob.as_bytes().to_vec()),
                media_type:  Set(item.media_type.clone()),
                mode:        Set(i64::from(item.mode)),
            }
            .insert(&transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(version_id)
    }

    pub async fn repository_snapshot(&self) -> Result<RepositorySnapshot> {
        let transaction = self.database.begin().await?;
        let package_models = package::Entity::find()
            .order_by_asc(package::Column::Id)
            .all(&transaction)
            .await?;
        let mut packages_by_id = BTreeMap::new();
        for model in package_models {
            validation::package_name(&model.name).map_err(|error| {
                PackageStoreError::InvalidSnapshot(format!(
                    "stored package name failed validation: {error}"
                ))
            })?;
            packages_by_id.insert(model.id, PackageKey {
                registry_id: RegistryId(model.registry_id),
                name:        model.name,
            });
        }

        let version_models = package_version::Entity::find()
            .order_by_asc(package_version::Column::Id)
            .all(&transaction)
            .await?;
        let mut package_by_version = BTreeMap::new();
        for model in &version_models {
            package_by_version.insert(model.id, model.package_id);
        }

        let dependency_models = dependency::Entity::find()
            .order_by_asc(dependency::Column::VersionId)
            .order_by_asc(dependency::Column::Ordinal)
            .all(&transaction)
            .await?;
        let mut dependencies = BTreeMap::<VersionId, Vec<SnapshotDependency>>::new();
        for model in dependency_models {
            let source_package_id = package_by_version.get(&model.version_id).ok_or_else(|| {
                PackageStoreError::InvalidSnapshot(
                    "dependency refers to a missing version".to_owned(),
                )
            })?;
            let source_package = packages_by_id.get(source_package_id).ok_or_else(|| {
                PackageStoreError::InvalidSnapshot("version refers to a missing package".to_owned())
            })?;
            validation::package_name(&model.package_name).map_err(|error| {
                PackageStoreError::InvalidSnapshot(format!(
                    "stored dependency name failed validation: {error}"
                ))
            })?;
            let parsed_requirement =
                node_semver::Range::parse(&model.requirement).map_err(|error| {
                    PackageStoreError::InvalidSnapshot(format!(
                        "stored dependency range `{}` failed validation: {error}",
                        model.requirement
                    ))
                })?;
            dependencies
                .entry(VersionId(model.version_id))
                .or_default()
                .push(SnapshotDependency {
                    kind: crate::DependencyKind::from_database(&model.kind)?,
                    alias: model.alias,
                    package_key: PackageKey {
                        registry_id: model
                            .target_registry_id
                            .map_or(source_package.registry_id, RegistryId),
                        name:        model.package_name,
                    },
                    requirement: model.requirement,
                    parsed_requirement,
                });
        }

        let mut snapshot = RepositorySnapshot::default();
        for model in version_models {
            let package_key = packages_by_id.get(&model.package_id).ok_or_else(|| {
                PackageStoreError::InvalidSnapshot("version refers to a missing package".to_owned())
            })?;
            let parsed_version = SemverVersion::parse(&model.version).map_err(|error| {
                PackageStoreError::InvalidSnapshot(format!(
                    "stored version `{}` failed validation: {error}",
                    model.version
                ))
            })?;
            snapshot
                .packages
                .entry(package_key.clone())
                .or_default()
                .push(SnapshotVersion {
                    id: VersionId(model.id),
                    raw_version: model.version,
                    parsed_version,
                    yanked_reason: model.yanked_reason,
                    dependencies: dependencies
                        .remove(&VersionId(model.id))
                        .unwrap_or_default(),
                });
        }
        if !dependencies.is_empty() {
            return Err(PackageStoreError::InvalidSnapshot(
                "dependencies refer to missing package versions".to_owned(),
            ));
        }
        transaction.commit().await?;
        Ok(snapshot)
    }
}

async fn initialize(database: &DatabaseConnection, in_memory: bool) -> Result<()> {
    if database.get_database_backend() != DbBackend::Sqlite {
        return Err(PackageStoreError::InvalidDatabasePath(
            "package stores require SQLite".to_owned(),
        ));
    }
    let application_id = pragma_i64(database, "application_id").await?;
    let user_version = pragma_i64(database, "user_version").await?;
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
    if application_id == 0 && !database_is_empty(database).await? {
        return Err(PackageStoreError::UnrecognizedDatabase);
    }

    install_tracking_table(&SchemaManager::new(database)).await?;
    let known_migrations = Migrator::migrations()
        .into_iter()
        .map(|migration| migration.name().to_owned())
        .collect::<BTreeSet<_>>();
    if let Some(unknown) = Migrator::get_migration_models(database)
        .await?
        .into_iter()
        .find(|migration| !known_migrations.contains(&migration.version))
    {
        return Err(PackageStoreError::UnknownMigration(unknown.version));
    }
    Migrator::up(database, None).await?;
    validate_schema(database).await?;
    execute_pragma(
        database,
        &format!("PRAGMA application_id = {APPLICATION_ID}"),
    )
    .await?;
    execute_pragma(database, &format!("PRAGMA user_version = {SCHEMA_VERSION}")).await?;
    if !in_memory {
        execute_pragma(database, "PRAGMA journal_mode = WAL").await?;
    }
    execute_pragma(database, "PRAGMA synchronous = FULL").await?;
    Ok(())
}

/// Refuse a store whose tables were rewritten under us. SQLite keeps the
/// `CREATE TABLE` text verbatim, so comparing it against the definition this
/// build ships covers columns, types, constraints and STRICT at once.
async fn validate_schema(database: &DatabaseConnection) -> Result<()> {
    for (table_name, table) in expected_tables() {
        validate_table_definition(database, table_name, &table).await?;
    }
    Ok(())
}

async fn validate_table_definition(
    database: &DatabaseConnection, table_name: &str, table: &TableCreateStatement,
) -> Result<()> {
    let actual = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?",
            [table_name.to_owned().into()],
        ))
        .await?
        .ok_or_else(|| schema_mismatch(table_name, "SeaQuery table definition", "missing"))?
        .try_get::<String>("", "sql")?;
    let expected = DbBackend::Sqlite.build(table).sql;
    let expected = normalized_table_sql(&expected);
    let actual = normalized_table_sql(&actual);
    if actual != expected {
        return Err(schema_mismatch(
            format!("{table_name} table definition"),
            expected,
            actual,
        ));
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

async fn execute_pragma(database: &DatabaseConnection, pragma: &str) -> Result<()> {
    database
        .execute_raw(Statement::from_string(DbBackend::Sqlite, pragma.to_owned()))
        .await?;
    Ok(())
}

async fn pragma_i64(database: &DatabaseConnection, name: &str) -> Result<i64> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("PRAGMA {name}"),
        ))
        .await?
        .ok_or_else(|| {
            PackageStoreError::InvalidSnapshot(format!("PRAGMA {name} returned no row"))
        })?;
    Ok(row.try_get_by_index(0)?)
}

async fn database_is_empty(database: &DatabaseConnection) -> Result<bool> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) AS count FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE \
             'sqlite_%'"
                .to_owned(),
        ))
        .await?
        .ok_or_else(|| {
            PackageStoreError::InvalidSnapshot("schema count returned no row".to_owned())
        })?;
    Ok(row.try_get::<i64>("", "count")? == 0)
}

async fn ensure_registry<C>(database: &C, id: RegistryId) -> Result<()>
where
    C: ConnectionTrait,
{
    if registry::Entity::find_by_id(id.0)
        .one(database)
        .await?
        .is_some()
    {
        Ok(())
    } else {
        Err(PackageStoreError::RegistryNotFound(id))
    }
}

async fn insert_blob_on<C>(database: &C, bytes: &[u8]) -> Result<BlobDigest>
where
    C: ConnectionTrait,
{
    let digest = BlobDigest::for_bytes(bytes);
    if let Some(existing) = blob::Entity::find_by_id(digest.as_bytes().to_vec())
        .one(database)
        .await?
    {
        verify_blob(digest, &existing.bytes)?;
        if existing.bytes.as_slice() != bytes {
            return Err(PackageStoreError::BlobCorrupt {
                expected: digest,
                actual:   BlobDigest::for_bytes(&existing.bytes),
            });
        }
        return Ok(digest);
    }
    let insertion = blob::ActiveModel {
        digest: Set(digest.as_bytes().to_vec()),
        bytes:  Set(bytes.to_vec()),
    }
    .insert(database)
    .await;
    match insertion {
        Ok(_) => {}
        Err(insert_error) => {
            if blob::Entity::find_by_id(digest.as_bytes().to_vec())
                .one(database)
                .await?
                .is_none()
            {
                return Err(insert_error.into());
            }
        }
    }
    let stored = read_blob_on(database, digest).await?;
    if stored.as_slice() != bytes {
        return Err(PackageStoreError::BlobCorrupt {
            expected: digest,
            actual:   BlobDigest::for_bytes(&stored),
        });
    }
    Ok(digest)
}

async fn read_blob_on<C>(database: &C, digest: BlobDigest) -> Result<Vec<u8>>
where
    C: ConnectionTrait,
{
    let model = blob::Entity::find_by_id(digest.as_bytes().to_vec())
        .one(database)
        .await?
        .ok_or(PackageStoreError::BlobNotFound(digest))?;
    verify_blob(digest, &model.bytes)?;
    Ok(model.bytes)
}

fn verify_blob(expected: BlobDigest, bytes: &[u8]) -> Result<()> {
    let actual = BlobDigest::for_bytes(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(PackageStoreError::BlobCorrupt { expected, actual })
    }
}

fn registry_identity(model: registry::Model, kind: &str) -> Result<RegistryId> {
    if model.kind == kind {
        Ok(RegistryId(model.id))
    } else {
        Err(PackageStoreError::InvalidRegistry(format!(
            "{} is already registered as `{}`, not `{kind}`",
            model.base_url, model.kind
        )))
    }
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
    use sea_orm::{ConnectionTrait as _, DbBackend, EntityTrait as _, Statement};

    use super::{PackageStore, blob};
    use crate::{NewRelease, PackageStoreError};

    #[tokio::test]
    async fn configures_connection_pragmas() -> Result<(), Box<dyn std::error::Error>> {
        let store = PackageStore::open_in_memory().await?;
        assert_eq!(super::pragma_i64(&store.database, "foreign_keys").await?, 1);
        assert_eq!(
            super::pragma_i64(&store.database, "trusted_schema").await?,
            0
        );
        assert_eq!(super::pragma_i64(&store.database, "synchronous").await?, 2);
        assert_eq!(
            super::pragma_i64(&store.database, "busy_timeout").await?,
            5_000
        );
        Ok(())
    }

    #[tokio::test]
    async fn detects_corrupt_blob_content() -> Result<(), Box<dyn std::error::Error>> {
        let store = PackageStore::open_in_memory().await?;
        let digest = store.insert_blob(b"original").await?;
        store
            .database
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE blob SET bytes = ? WHERE digest = ?",
                [
                    b"tampered".as_slice().into(),
                    digest.as_bytes().as_slice().into(),
                ],
            ))
            .await?;
        // `insert_blob_on` re-reads and hash-verifies what is already stored,
        // so the tampered row is caught on the way in just as it was on
        // the way out.
        assert!(matches!(
            store.insert_blob(b"original").await,
            Err(PackageStoreError::BlobCorrupt { expected, .. }) if expected == digest
        ));
        assert!(
            blob::Entity::find_by_id(digest.as_bytes().to_vec())
                .one(&store.database)
                .await?
                .is_some()
        );
        Ok(())
    }

    #[tokio::test]
    async fn release_transaction_rolls_back_package_on_mid_commit_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = PackageStore::open_in_memory().await?;
        let registry = store.add_registry("jsr", "https://jsr.example/").await?;
        store
            .database
            .execute_unprepared(
                "CREATE TRIGGER fail_package_version BEFORE INSERT ON package_version BEGIN \
                 SELECT RAISE(ABORT, 'forced test failure'); END",
            )
            .await?;

        assert!(
            store
                .insert_release(&NewRelease::new(registry, "rollback", "1.0.0"))
                .await
                .is_err()
        );
        let packages = store
            .database
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM package".to_owned(),
            ))
            .await?
            .ok_or("count returned no row")?
            .try_get::<i64>("", "count")?;
        assert_eq!(packages, 0);
        Ok(())
    }
}
