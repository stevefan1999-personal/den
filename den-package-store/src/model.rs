use std::{collections::BTreeMap, fmt};

use sha2::{Digest as _, Sha256};

use crate::{PackageStoreError, Result};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RegistryId(pub i64);

impl fmt::Display for RegistryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VersionId(pub i64);

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlobDigest(pub [u8; Self::LEN]);

impl BlobDigest {
    pub const LEN: usize = 32;

    #[must_use]
    pub fn for_bytes(bytes: &[u8]) -> Self { Self(Sha256::digest(bytes).into()) }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::LEN] { &self.0 }

    pub(crate) fn from_database(bytes: Vec<u8>) -> Result<Self> {
        let digest = <[u8; Self::LEN]>::try_from(bytes).map_err(|bytes| {
            PackageStoreError::InvalidSnapshot(format!(
                "blob digest has {} bytes instead of {}",
                bytes.len(),
                Self::LEN
            ))
        })?;
        Ok(Self(digest))
    }
}

impl fmt::Debug for BlobDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { fmt::Display::fmt(self, f) }
}

impl fmt::Display for BlobDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Registry {
    pub id:       RegistryId,
    pub kind:     String,
    pub base_url: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependencyKind {
    Normal,
    Optional,
    Peer,
    PeerOptional,
}

impl DependencyKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Optional => "optional",
            Self::Peer => "peer",
            Self::PeerOptional => "peer_optional",
        }
    }

    pub(crate) fn from_database(value: &str) -> Result<Self> {
        match value {
            "normal" => Ok(Self::Normal),
            "optional" => Ok(Self::Optional),
            "peer" => Ok(Self::Peer),
            "peer_optional" => Ok(Self::PeerOptional),
            _ => {
                Err(PackageStoreError::InvalidSnapshot(format!(
                    "unknown dependency kind `{value}`"
                )))
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewDependency {
    pub kind:               DependencyKind,
    pub target_registry_id: Option<RegistryId>,
    pub package:            String,
    pub requirement:        String,
    pub alias:              Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewExport {
    pub name:   String,
    pub target: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewPackageFile {
    pub path:       String,
    pub blob:       BlobDigest,
    pub media_type: Option<String>,
    pub mode:       u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewRelease {
    pub registry_id:   RegistryId,
    pub package:       String,
    pub version:       String,
    pub published_at:  Option<i64>,
    pub yanked_reason: Option<String>,
    pub manifest:      Vec<u8>,
    pub dependencies:  Vec<NewDependency>,
    pub exports:       Vec<NewExport>,
    pub files:         Vec<NewPackageFile>,
}

impl NewRelease {
    #[must_use]
    pub fn new<P: Into<String>, V: Into<String>>(
        registry_id: RegistryId, package: P, version: V,
    ) -> Self {
        Self {
            registry_id,
            package: package.into(),
            version: version.into(),
            published_at: None,
            yanked_reason: None,
            manifest: Vec::new(),
            dependencies: Vec::new(),
            exports: Vec::new(),
            files: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageVersion {
    pub id:            VersionId,
    pub version:       String,
    pub published_at:  Option<i64>,
    pub yanked_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Package {
    pub registry_id: RegistryId,
    pub name:        String,
    pub versions:    Vec<PackageVersion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Module {
    pub registry_id: RegistryId,
    pub package:     String,
    pub version:     String,
    pub path:        String,
    pub digest:      BlobDigest,
    pub media_type:  Option<String>,
    pub bytes:       Vec<u8>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PackageKey {
    pub registry_id: RegistryId,
    pub name:        String,
}

impl fmt::Display for PackageKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.registry_id, self.name)
    }
}

#[derive(Clone, Debug)]
pub struct SnapshotDependency {
    pub kind:               DependencyKind,
    pub alias:              Option<String>,
    pub package_key:        PackageKey,
    pub requirement:        String,
    pub parsed_requirement: node_semver::Range,
}

#[derive(Clone, Debug)]
pub struct SnapshotVersion {
    pub id:             VersionId,
    pub raw_version:    String,
    pub parsed_version: node_semver::Version,
    pub yanked_reason:  Option<String>,
    pub dependencies:   Vec<SnapshotDependency>,
}

#[derive(Clone, Debug, Default)]
pub struct RepositorySnapshot {
    pub(crate) packages: BTreeMap<PackageKey, Vec<SnapshotVersion>>,
}

impl RepositorySnapshot {
    #[must_use]
    pub fn package_count(&self) -> usize { self.packages.len() }

    #[must_use]
    pub fn version_count(&self) -> usize { self.packages.values().map(Vec::len).sum() }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedPackage {
    pub registry_id: RegistryId,
    pub package:     String,
    pub version_id:  VersionId,
    pub version:     String,
}

/// One application-visible package root selected by the solver.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResolvedRootEdge {
    pub specifier:         String,
    pub requirement:       String,
    pub target:            PackageKey,
    pub target_version_id: VersionId,
}

/// One exact normal-dependency edge between selected package versions.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResolvedDependencyEdge {
    pub importer:            PackageKey,
    pub importer_version_id: VersionId,
    pub specifier:           String,
    pub requirement:         String,
    pub target:              PackageKey,
    pub target_version_id:   VersionId,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SolveResult {
    pub packages:     Vec<ResolvedPackage>,
    pub roots:        Vec<ResolvedRootEdge>,
    pub dependencies: Vec<ResolvedDependencyEdge>,
}
