use crate::{BlobDigest, RegistryId};

pub type Result<T> = std::result::Result<T, PackageStoreError>;

#[derive(Debug, thiserror::Error)]
pub enum PackageStoreError {
    #[error("package store belongs to another application (application_id {actual:#x})")]
    ForeignDatabase { actual: i64 },
    #[error("package store schema {actual} is newer than supported schema {supported}")]
    SchemaTooNew { actual: i64, supported: i64 },
    #[error("package store contains unknown migration `{0}`")]
    UnknownMigration(String),
    #[error("database is not an empty or recognized Den package store")]
    UnrecognizedDatabase,
    #[error("package-store schema mismatch at `{object}`: expected {expected}, found {actual}")]
    SchemaMismatch {
        object:   String,
        expected: String,
        actual:   String,
    },
    #[error("invalid package-store database path: {0}")]
    InvalidDatabasePath(String),
    #[error("invalid registry: {0}")]
    InvalidRegistry(String),
    #[error("registry {0} does not exist")]
    RegistryNotFound(RegistryId),
    #[error("invalid package name `{name}`: {reason}")]
    InvalidPackageName {
        name:   String,
        reason: &'static str,
    },
    #[error("invalid module path `{path}`: {reason}")]
    InvalidModulePath {
        path:   String,
        reason: &'static str,
    },
    #[error("invalid export name `{name}`")]
    InvalidExportName { name: String },
    #[error("export `{name}` targets missing package file `{target}`")]
    MissingExportTarget { name: String, target: String },
    #[error("invalid semantic version `{version}`: {reason}")]
    InvalidVersion { version: String, reason: String },
    #[error("invalid semantic version range `{range}`: {reason}")]
    InvalidVersionRange { range: String, reason: String },
    #[error("duplicate {kind} `{value}` in release")]
    DuplicateReleaseEntry { kind: &'static str, value: String },
    #[error("release `{package}@{version}` already exists")]
    ReleaseExists { package: String, version: String },
    #[error("blob {0} does not exist")]
    BlobNotFound(BlobDigest),
    #[error("blob {expected} is corrupt (content hashes to {actual})")]
    BlobCorrupt {
        expected: BlobDigest,
        actual:   BlobDigest,
    },
    #[error("package snapshot is invalid: {0}")]
    InvalidSnapshot(String),
    #[error("dependency resolution failed:\n{0}")]
    Conflict(String),
    #[error("dependency resolution was cancelled")]
    Cancelled,
    #[error(transparent)]
    Database(#[from] sea_orm::DbErr),
}
