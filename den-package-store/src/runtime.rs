use std::{collections::BTreeMap, sync::Arc};

use sea_orm::{
    ColumnTrait as _, EntityTrait as _, PaginatorTrait as _, QueryFilter as _, QueryOrder as _,
    QuerySelect as _, TransactionTrait as _,
    sea_query::{Alias, Expr, Func},
};
use url::Url;

use crate::{
    BlobDigest, DependencyKind, PackageKey, PackageStore, PackageStoreError, RegistryId,
    ResolvedDependencyEdge, ResolvedPackage, ResolvedRootEdge, SolveResult, VersionId,
    entity::{blob, dependency, package, package_export, package_file, package_version, registry},
    validation,
};

pub type HydrationResult<T> = std::result::Result<T, PackageHydrationError>;
pub type ResolutionResult<T> = std::result::Result<T, PackageResolutionError>;

/// Resource limits applied while copying a solved package graph out of SQLite.
/// `max_bytes` includes module bodies and manifests verified during hydration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HydrationLimits {
    pub max_packages:         u64,
    pub max_root_edges:       u64,
    pub max_dependency_edges: u64,
    pub max_exports:          u64,
    pub max_files:            u64,
    pub max_bytes:            u64,
}

impl HydrationLimits {
    #[must_use]
    pub const fn new(max_files: u64, max_bytes: u64) -> Self {
        Self {
            max_packages: 10_000,
            max_root_edges: 10_000,
            max_dependency_edges: 100_000,
            max_exports: 100_000,
            max_files,
            max_bytes,
        }
    }
}

impl Default for HydrationLimits {
    fn default() -> Self { Self::new(100_000, 512 * 1024 * 1024) }
}

/// Failures while turning a solver result into one immutable runtime view.
#[derive(Debug, thiserror::Error)]
pub enum PackageHydrationError {
    #[error("selected package `{package}` from registry {registry_id} appears more than once")]
    DuplicateSelectedPackage {
        registry_id: RegistryId,
        package:     String,
    },
    #[error("selected version id {} does not exist", .0.0)]
    SelectedVersionNotFound(VersionId),
    #[error("selected version id {} refers to a missing package row", .version_id.0)]
    SelectedPackageNotFound { version_id: VersionId },
    #[error("selected registry {0} does not exist")]
    SelectedRegistryNotFound(RegistryId),
    #[error("selected version id {} is `{actual}`, not `{expected}`", .version_id.0)]
    SelectedVersionMismatch {
        version_id: VersionId,
        expected:   String,
        actual:     String,
    },
    #[error("export `{name}` of `{package}@{version}` targets missing file `{target}`")]
    DanglingExport {
        package: String,
        version: String,
        name:    String,
        target:  String,
    },
    #[error("two selected modules have the canonical URL `{0}`")]
    DuplicateModuleUrl(String),
    #[error("invalid solved package edge: {0}")]
    InvalidSolutionEdge(String),
    #[error("hydrating {attempted} packages exceeds the configured limit of {limit}")]
    PackageLimitExceeded { limit: u64, attempted: u64 },
    #[error("hydrated package count overflowed")]
    PackageCountOverflow,
    #[error("hydrating {attempted} root edges exceeds the configured limit of {limit}")]
    RootEdgeLimitExceeded { limit: u64, attempted: u64 },
    #[error("hydrated root-edge count overflowed")]
    RootEdgeCountOverflow,
    #[error("hydrating {attempted} dependency edges exceeds the configured limit of {limit}")]
    DependencyLimitExceeded { limit: u64, attempted: u64 },
    #[error("hydrated dependency-edge count overflowed")]
    DependencyCountOverflow,
    #[error("hydrating {attempted} exports exceeds the configured limit of {limit}")]
    ExportLimitExceeded { limit: u64, attempted: u64 },
    #[error("hydrated export count overflowed")]
    ExportCountOverflow,
    #[error("hydrating {attempted} files exceeds the configured limit of {limit}")]
    FileLimitExceeded { limit: u64, attempted: u64 },
    #[error("hydrated file count overflowed")]
    FileCountOverflow,
    #[error("hydrating {attempted} bytes exceeds the configured limit of {limit}")]
    ByteLimitExceeded { limit: u64, attempted: u64 },
    #[error("hydrated byte count overflowed")]
    ByteCountOverflow,
    #[error(transparent)]
    InvalidStore(#[from] PackageStoreError),
    #[error(transparent)]
    Database(#[from] sea_orm::DbErr),
}

/// Deterministic resolution failures for the deliberately flat package graph.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PackageResolutionError {
    #[error("specifier `{specifier}` is not a package specifier")]
    UnsupportedSpecifier { specifier: String },
    #[error("canonical package URL `{specifier}` cannot be imported directly")]
    DirectCanonicalImport { specifier: String },
    #[error("invalid bare package specifier `{specifier}`")]
    InvalidBareSpecifier { specifier: String },
    #[error("package `{package}` is not selected")]
    PackageNotSelected { package: String },
    #[error("package `{package}` is selected from more than one registry: {registries}")]
    AmbiguousPackage {
        package:    String,
        registries: String,
    },
    #[error("package `{package}` does not export `{name}`")]
    MissingExport { package: String, name: String },
    #[error("package `{importer}` does not declare dependency `{package}`")]
    UndeclaredDependency { importer: String, package: String },
    #[error("`{specifier}` cannot be resolved relative to non-package module `{base}`")]
    InvalidPackageBase {
        base:      String,
        specifier: String,
    },
    #[error("relative specifier `{specifier}` escapes package module `{base}`")]
    PackageTraversal {
        base:      String,
        specifier: String,
    },
    #[error("package module `{0}` does not exist")]
    ModuleNotFound(String),
}

#[derive(Clone, Debug)]
struct SnapshotPackage {
    registry:     String,
    exports:      BTreeMap<String, String>,
    files:        BTreeMap<String, String>,
    dependencies: BTreeMap<String, Vec<PackageKey>>,
}

#[derive(Debug)]
struct HydrationBudget {
    limits:       HydrationLimits,
    dependencies: u64,
    exports:      u64,
    files:        u64,
    bytes:        u64,
}

impl HydrationBudget {
    const fn new(limits: HydrationLimits) -> Self {
        Self {
            limits,
            dependencies: 0,
            exports: 0,
            files: 0,
            bytes: 0,
        }
    }

    fn check_packages(&self, count: usize) -> HydrationResult<()> {
        let attempted =
            u64::try_from(count).map_err(|_error| PackageHydrationError::PackageCountOverflow)?;
        if attempted > self.limits.max_packages {
            return Err(PackageHydrationError::PackageLimitExceeded {
                limit: self.limits.max_packages,
                attempted,
            });
        }
        Ok(())
    }

    fn check_root_edges(&self, count: usize) -> HydrationResult<()> {
        let attempted =
            u64::try_from(count).map_err(|_error| PackageHydrationError::RootEdgeCountOverflow)?;
        if attempted > self.limits.max_root_edges {
            return Err(PackageHydrationError::RootEdgeLimitExceeded {
                limit: self.limits.max_root_edges,
                attempted,
            });
        }
        Ok(())
    }

    fn check_solved_dependencies(&self, count: usize) -> HydrationResult<()> {
        let attempted = u64::try_from(count)
            .map_err(|_error| PackageHydrationError::DependencyCountOverflow)?;
        if attempted > self.limits.max_dependency_edges {
            return Err(PackageHydrationError::DependencyLimitExceeded {
                limit: self.limits.max_dependency_edges,
                attempted,
            });
        }
        Ok(())
    }

    fn add_dependencies(&mut self, count: u64) -> HydrationResult<()> {
        let attempted = self
            .dependencies
            .checked_add(count)
            .ok_or(PackageHydrationError::DependencyCountOverflow)?;
        if attempted > self.limits.max_dependency_edges {
            return Err(PackageHydrationError::DependencyLimitExceeded {
                limit: self.limits.max_dependency_edges,
                attempted,
            });
        }
        self.dependencies = attempted;
        Ok(())
    }

    fn add_exports(&mut self, count: u64) -> HydrationResult<()> {
        let attempted = self
            .exports
            .checked_add(count)
            .ok_or(PackageHydrationError::ExportCountOverflow)?;
        if attempted > self.limits.max_exports {
            return Err(PackageHydrationError::ExportLimitExceeded {
                limit: self.limits.max_exports,
                attempted,
            });
        }
        self.exports = attempted;
        Ok(())
    }

    fn add_files(&mut self, count: u64) -> HydrationResult<()> {
        let attempted = self
            .files
            .checked_add(count)
            .ok_or(PackageHydrationError::FileCountOverflow)?;
        if attempted > self.limits.max_files {
            return Err(PackageHydrationError::FileLimitExceeded {
                limit: self.limits.max_files,
                attempted,
            });
        }
        self.files = attempted;
        Ok(())
    }

    fn add_bytes(&mut self, count: u64) -> HydrationResult<()> {
        let attempted = self
            .bytes
            .checked_add(count)
            .ok_or(PackageHydrationError::ByteCountOverflow)?;
        if attempted > self.limits.max_bytes {
            return Err(PackageHydrationError::ByteLimitExceeded {
                limit: self.limits.max_bytes,
                attempted,
            });
        }
        self.bytes = attempted;
        Ok(())
    }
}

/// One verified module body loaded from the SQLite CAS.
#[derive(Clone, Debug)]
pub struct PackageModule {
    package_key: PackageKey,
    path:        String,
    url:         String,
    media_type:  Option<String>,
    bytes:       Arc<[u8]>,
}

impl PackageModule {
    #[must_use]
    pub fn url(&self) -> &str { &self.url }

    #[must_use]
    pub fn path(&self) -> &str { &self.path }

    #[must_use]
    pub fn media_type(&self) -> Option<&str> { self.media_type.as_deref() }

    #[must_use]
    pub fn bytes(&self) -> &[u8] { &self.bytes }
}

/// A read-only, database-free module graph for one flat solver result.
#[derive(Clone, Debug, Default)]
pub struct PackageModuleSnapshot {
    packages: BTreeMap<PackageKey, SnapshotPackage>,
    roots:    BTreeMap<String, Vec<PackageKey>>,
    modules:  BTreeMap<String, PackageModule>,
}

impl PackageModuleSnapshot {
    #[must_use]
    pub fn package_count(&self) -> usize { self.packages.len() }

    #[must_use]
    pub fn module_count(&self) -> usize { self.modules.len() }

    /// Load a canonical URL previously returned by [`Self::resolve`] or
    /// [`Self::resolve_if_claimed`]. Authored `den-pkg:` imports are rejected
    /// by the resolver so this loader lookup cannot bypass solution edges.
    #[must_use]
    pub fn module(&self, url: &str) -> Option<&PackageModule> { self.modules.get(url) }

    /// Resolve a package specifier, returning `None` when this snapshot does
    /// not claim it and another resolver may retry it.
    ///
    /// Non-package importers can see only explicit roots. Package importers
    /// can see only themselves and their exact normal-dependency edges.
    /// Authored canonical URLs are claimed but rejected; only canonical URLs
    /// returned by this method should reach [`Self::module`].
    #[must_use]
    pub fn resolve_if_claimed(
        &self, base: &str, specifier: &str,
    ) -> Option<ResolutionResult<String>> {
        if specifier.starts_with("den-pkg:") {
            return Some(Err(PackageResolutionError::DirectCanonicalImport {
                specifier: specifier.to_owned(),
            }));
        }
        if is_relative(specifier) {
            return base
                .starts_with("den-pkg:")
                .then(|| self.resolve_relative(base, specifier));
        }
        if specifier.starts_with('/') || Url::parse(specifier).is_ok() {
            return base.starts_with("den-pkg:").then(|| {
                Err(PackageResolutionError::UnsupportedSpecifier {
                    specifier: specifier.to_owned(),
                })
            });
        }

        let (package_name, export_name) = match split_bare_specifier(specifier) {
            Ok(parts) => parts,
            Err(error) if base.starts_with("den-pkg:") => return Some(Err(error)),
            Err(_error) => return None,
        };
        if let Some(module) = self.modules.get(base) {
            return Some(self.resolve_from_package(module, package_name, export_name));
        }
        if base.starts_with("den-pkg:") {
            return Some(Err(PackageResolutionError::InvalidPackageBase {
                base:      base.to_owned(),
                specifier: specifier.to_owned(),
            }));
        }
        self.roots
            .get(&package_name)
            .map(|keys| self.resolve_export(keys, package_name, export_name))
    }

    /// Resolve a specifier claimed by this snapshot.
    ///
    /// Prefer [`Self::resolve_if_claimed`] in resolver chains so an unclaimed
    /// bare file name remains retryable by later resolvers.
    pub fn resolve(&self, base: &str, specifier: &str) -> ResolutionResult<String> {
        self.resolve_if_claimed(base, specifier).unwrap_or_else(|| {
            Err(PackageResolutionError::UnsupportedSpecifier {
                specifier: specifier.to_owned(),
            })
        })
    }

    fn resolve_from_package(
        &self, module: &PackageModule, package_name: String, export_name: String,
    ) -> ResolutionResult<String> {
        let package = self.packages.get(&module.package_key).ok_or_else(|| {
            PackageResolutionError::InvalidPackageBase {
                base:      module.url.clone(),
                specifier: package_name.clone(),
            }
        })?;
        if package_name == module.package_key.name {
            return self.resolve_export(
                std::slice::from_ref(&module.package_key),
                package_name,
                export_name,
            );
        }
        let keys = package.dependencies.get(&package_name).ok_or_else(|| {
            PackageResolutionError::UndeclaredDependency {
                importer: module.package_key.name.clone(),
                package:  package_name.clone(),
            }
        })?;
        self.resolve_export(keys, package_name, export_name)
    }

    fn resolve_export(
        &self, keys: &[PackageKey], package_name: String, export_name: String,
    ) -> ResolutionResult<String> {
        if keys.len() != 1 {
            let mut registries = keys
                .iter()
                .filter_map(|key| self.packages.get(key))
                .map(|package| package.registry.clone())
                .collect::<Vec<_>>();
            registries.sort();
            return Err(PackageResolutionError::AmbiguousPackage {
                package:    package_name,
                registries: registries.join(", "),
            });
        }
        let key = keys.first().ok_or_else(|| {
            PackageResolutionError::PackageNotSelected {
                package: package_name.clone(),
            }
        })?;
        let package = self.packages.get(key).ok_or_else(|| {
            PackageResolutionError::PackageNotSelected {
                package: package_name.clone(),
            }
        })?;
        package
            .exports
            .get(&export_name)
            .cloned()
            .ok_or(PackageResolutionError::MissingExport {
                package: package_name,
                name:    export_name,
            })
    }

    fn resolve_relative(&self, base: &str, specifier: &str) -> ResolutionResult<String> {
        let module = self.modules.get(base).ok_or_else(|| {
            PackageResolutionError::InvalidPackageBase {
                base:      base.to_owned(),
                specifier: specifier.to_owned(),
            }
        })?;
        if specifier.contains(['?', '#', '\\', '\0']) {
            return Err(PackageResolutionError::ModuleNotFound(specifier.to_owned()));
        }

        let mut path = module
            .path
            .rsplit_once('/')
            .map_or_else(Vec::new, |(parent, _file)| parent.split('/').collect());
        for segment in specifier.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    if path.pop().is_none() {
                        return Err(PackageResolutionError::PackageTraversal {
                            base:      base.to_owned(),
                            specifier: specifier.to_owned(),
                        });
                    }
                }
                segment => path.push(segment),
            }
        }
        let path = path.join("/");
        let package = self.packages.get(&module.package_key).ok_or_else(|| {
            PackageResolutionError::InvalidPackageBase {
                base:      base.to_owned(),
                specifier: specifier.to_owned(),
            }
        })?;
        package.files.get(&path).cloned().ok_or_else(|| {
            PackageResolutionError::ModuleNotFound(format!("{path} ({specifier} from {base})"))
        })
    }
}

impl PackageStore {
    /// Materialize all selected modules and verify their CAS content under one
    /// SeaORM read transaction. The returned snapshot performs no I/O.
    pub async fn hydrate_modules(
        &self, solved: &SolveResult,
    ) -> HydrationResult<PackageModuleSnapshot> {
        self.hydrate_modules_with_limits(solved, HydrationLimits::default())
            .await
    }

    /// Materialize modules with explicit finite file and verified-byte limits.
    pub async fn hydrate_modules_with_limits(
        &self, solved: &SolveResult, limits: HydrationLimits,
    ) -> HydrationResult<PackageModuleSnapshot> {
        let mut budget = HydrationBudget::new(limits);
        budget.check_packages(solved.packages.len())?;
        budget.check_root_edges(solved.roots.len())?;
        budget.check_solved_dependencies(solved.dependencies.len())?;
        let selected = selected_packages(solved)?;
        let transaction = self.database().begin().await?;
        let mut snapshot = PackageModuleSnapshot::default();

        let mut expected_dependency_edges = Vec::new();
        for (key, selected_package) in &selected {
            let version_model = package_version::Entity::find_by_id(selected_package.version_id.0)
                .one(&transaction)
                .await?
                .ok_or(PackageHydrationError::SelectedVersionNotFound(
                    selected_package.version_id,
                ))?;
            let package_model = package::Entity::find_by_id(version_model.package_id)
                .one(&transaction)
                .await?
                .ok_or(PackageHydrationError::SelectedPackageNotFound {
                    version_id: selected_package.version_id,
                })?;
            let actual = format!(
                "{}:{}@{}",
                package_model.registry_id, package_model.name, version_model.version
            );
            let expected = format!(
                "{}:{}@{}",
                selected_package.registry_id, selected_package.package, selected_package.version
            );
            if package_model.registry_id != selected_package.registry_id.0
                || package_model.name != selected_package.package
                || version_model.version != selected_package.version
            {
                return Err(PackageHydrationError::SelectedVersionMismatch {
                    version_id: selected_package.version_id,
                    expected,
                    actual,
                });
            }
            validation::package_name(&package_model.name)?;
            let parsed_version =
                node_semver::Version::parse(&version_model.version).map_err(|error| {
                    PackageStoreError::InvalidSnapshot(format!(
                        "stored version `{}` failed validation: {error}",
                        version_model.version
                    ))
                })?;
            if parsed_version.to_string() != version_model.version {
                return Err(PackageStoreError::InvalidSnapshot(format!(
                    "stored version `{}` is not canonical (`{parsed_version}`)",
                    version_model.version
                ))
                .into());
            }
            let registry_model = registry::Entity::find_by_id(package_model.registry_id)
                .one(&transaction)
                .await?
                .ok_or(PackageHydrationError::SelectedRegistryNotFound(
                    selected_package.registry_id,
                ))?;

            let dependency_count = dependency::Entity::find()
                .filter(dependency::Column::VersionId.eq(version_model.id))
                .count(&transaction)
                .await?;
            budget.add_dependencies(dependency_count)?;
            let dependency_models = dependency::Entity::find()
                .filter(dependency::Column::VersionId.eq(version_model.id))
                .order_by_asc(dependency::Column::Ordinal)
                .all(&transaction)
                .await?;
            for dependency in dependency_models {
                let kind = DependencyKind::from_database(&dependency.kind)?;
                if kind != DependencyKind::Normal || dependency.alias.is_some() {
                    return Err(PackageHydrationError::InvalidSolutionEdge(format!(
                        "selected package `{key}` contains unsupported {} dependency `{}`",
                        kind.as_str(),
                        dependency.package_name
                    )));
                }
                validation::package_name(&dependency.package_name)?;
                let target = PackageKey {
                    registry_id: dependency
                        .target_registry_id
                        .map_or(key.registry_id, RegistryId),
                    name:        dependency.package_name,
                };
                let selected_target = selected.get(&target).ok_or_else(|| {
                    PackageHydrationError::InvalidSolutionEdge(format!(
                        "dependency `{target}` required by `{key}` is not selected"
                    ))
                })?;
                expected_dependency_edges.push(ResolvedDependencyEdge {
                    importer: key.clone(),
                    importer_version_id: selected_package.version_id,
                    specifier: target.name.clone(),
                    requirement: dependency.requirement,
                    target,
                    target_version_id: selected_target.version_id,
                });
            }

            // Manifests are CAS content too even though the runtime loader does
            // not otherwise retain them.
            read_verified_blob(&transaction, &version_model.manifest_digest, &mut budget).await?;

            let file_count = package_file::Entity::find()
                .filter(package_file::Column::VersionId.eq(version_model.id))
                .count(&transaction)
                .await?;
            budget.add_files(file_count)?;
            let file_models = package_file::Entity::find()
                .filter(package_file::Column::VersionId.eq(version_model.id))
                .order_by_asc(package_file::Column::Path)
                .all(&transaction)
                .await?;
            let mut files = BTreeMap::new();
            for file in file_models {
                validation::module_path(&file.path)?;
                let bytes =
                    read_verified_blob(&transaction, &file.blob_digest, &mut budget).await?;
                let url = module_url(
                    &registry_model,
                    &package_model.name,
                    &version_model.version,
                    &file.path,
                )?;
                let module = PackageModule {
                    package_key: key.clone(),
                    path:        file.path.clone(),
                    url:         url.clone(),
                    media_type:  file.media_type,
                    bytes:       bytes.into(),
                };
                if snapshot.modules.insert(url.clone(), module).is_some() {
                    return Err(PackageHydrationError::DuplicateModuleUrl(url));
                }
                files.insert(file.path, url);
            }

            let export_count = package_export::Entity::find()
                .filter(package_export::Column::VersionId.eq(version_model.id))
                .count(&transaction)
                .await?;
            budget.add_exports(export_count)?;
            let export_models = package_export::Entity::find()
                .filter(package_export::Column::VersionId.eq(version_model.id))
                .order_by_asc(package_export::Column::Name)
                .all(&transaction)
                .await?;
            let mut exports = BTreeMap::new();
            for export in export_models {
                validation::export_name(&export.name)?;
                validation::module_path(&export.target_path)?;
                let target = files.get(&export.target_path).cloned().ok_or_else(|| {
                    PackageHydrationError::DanglingExport {
                        package: package_model.name.clone(),
                        version: version_model.version.clone(),
                        name:    export.name.clone(),
                        target:  export.target_path.clone(),
                    }
                })?;
                exports.insert(export.name, target);
            }

            let registry_name = format!("{}:{}", registry_model.kind, registry_model.base_url);
            snapshot.packages.insert(key.clone(), SnapshotPackage {
                registry: registry_name,
                exports,
                files,
                dependencies: BTreeMap::new(),
            });
        }
        expected_dependency_edges.sort();
        expected_dependency_edges.dedup();
        let mut solved_dependency_edges = solved.dependencies.clone();
        solved_dependency_edges.sort();
        solved_dependency_edges.dedup();
        if solved_dependency_edges != expected_dependency_edges {
            return Err(PackageHydrationError::InvalidSolutionEdge(
                "dependency edges do not match selected release metadata".to_owned(),
            ));
        }
        hydrate_solution_edges(&selected, solved, &mut snapshot)?;
        transaction.commit().await?;
        Ok(snapshot)
    }
}

fn selected_packages(
    solved: &SolveResult,
) -> HydrationResult<BTreeMap<PackageKey, ResolvedPackage>> {
    let mut selected = BTreeMap::new();
    for package in &solved.packages {
        validation::package_name(&package.package)?;
        let key = PackageKey {
            registry_id: package.registry_id,
            name:        package.package.clone(),
        };
        if selected.insert(key, package.clone()).is_some() {
            return Err(PackageHydrationError::DuplicateSelectedPackage {
                registry_id: package.registry_id,
                package:     package.package.clone(),
            });
        }
    }
    Ok(selected)
}

fn hydrate_solution_edges(
    selected: &BTreeMap<PackageKey, ResolvedPackage>, solved: &SolveResult,
    snapshot: &mut PackageModuleSnapshot,
) -> HydrationResult<()> {
    for edge in &solved.roots {
        validate_root_edge(selected, edge)?;
        snapshot
            .roots
            .entry(edge.specifier.clone())
            .or_default()
            .push(edge.target.clone());
    }
    for edge in &solved.dependencies {
        validate_dependency_edge(selected, edge)?;
        let package = snapshot.packages.get_mut(&edge.importer).ok_or_else(|| {
            PackageHydrationError::InvalidSolutionEdge(format!(
                "dependency importer `{}` has no hydrated package",
                edge.importer
            ))
        })?;
        package
            .dependencies
            .entry(edge.specifier.clone())
            .or_default()
            .push(edge.target.clone());
    }

    for keys in snapshot.roots.values_mut() {
        keys.sort();
        keys.dedup();
    }
    for package in snapshot.packages.values_mut() {
        for keys in package.dependencies.values_mut() {
            keys.sort();
            keys.dedup();
        }
    }
    Ok(())
}

fn validate_root_edge(
    selected: &BTreeMap<PackageKey, ResolvedPackage>, edge: &ResolvedRootEdge,
) -> HydrationResult<()> {
    validation::package_name(&edge.specifier)?;
    let target = validate_edge_endpoint(
        selected,
        &edge.target,
        edge.target_version_id,
        "root target",
    )?;
    validate_requirement(&edge.requirement, target, "root")
}

fn validate_dependency_edge(
    selected: &BTreeMap<PackageKey, ResolvedPackage>, edge: &ResolvedDependencyEdge,
) -> HydrationResult<()> {
    validation::package_name(&edge.specifier)?;
    if edge.specifier != edge.target.name {
        return Err(PackageHydrationError::InvalidSolutionEdge(format!(
            "dependency alias `{}` for `{}` is unsupported",
            edge.specifier, edge.target
        )));
    }
    validate_edge_endpoint(
        selected,
        &edge.importer,
        edge.importer_version_id,
        "dependency importer",
    )?;
    let target = validate_edge_endpoint(
        selected,
        &edge.target,
        edge.target_version_id,
        "dependency target",
    )?;
    validate_requirement(&edge.requirement, target, "dependency")
}

fn validate_edge_endpoint<'a>(
    selected: &'a BTreeMap<PackageKey, ResolvedPackage>, package: &PackageKey,
    version_id: VersionId, role: &str,
) -> HydrationResult<&'a ResolvedPackage> {
    let selected_package = selected.get(package).ok_or_else(|| {
        PackageHydrationError::InvalidSolutionEdge(format!("{role} `{package}` is not selected"))
    })?;
    if selected_package.version_id != version_id {
        return Err(PackageHydrationError::InvalidSolutionEdge(format!(
            "{role} `{package}` references version id {} instead of selected version id {}",
            version_id.0, selected_package.version_id.0
        )));
    }
    Ok(selected_package)
}

fn validate_requirement(
    requirement: &str, target: &ResolvedPackage, role: &str,
) -> HydrationResult<()> {
    let range = node_semver::Range::parse(requirement).map_err(|error| {
        PackageHydrationError::InvalidSolutionEdge(format!(
            "{role} requirement `{requirement}` is invalid: {error}"
        ))
    })?;
    let version = node_semver::Version::parse(&target.version).map_err(|error| {
        PackageHydrationError::InvalidSolutionEdge(format!(
            "{role} target version `{}` is invalid: {error}",
            target.version
        ))
    })?;
    if !version.satisfies(&range) {
        return Err(PackageHydrationError::InvalidSolutionEdge(format!(
            "{role} target `{}@{}` does not satisfy `{requirement}`",
            target.package, target.version
        )));
    }
    Ok(())
}

async fn read_verified_blob<C>(
    database: &C, raw_digest: &[u8], budget: &mut HydrationBudget,
) -> HydrationResult<Vec<u8>>
where
    C: sea_orm::ConnectionTrait,
{
    let digest = BlobDigest::from_database(raw_digest.to_vec())?;
    let byte_count = blob::Entity::find_by_id(raw_digest.to_vec())
        .select_only()
        .expr_as(
            Func::cust(Alias::new("length")).arg(Expr::col(blob::Column::Bytes)),
            "byte_count",
        )
        .into_tuple::<i64>()
        .one(database)
        .await?
        .ok_or(PackageStoreError::BlobNotFound(digest))?;
    budget.add_bytes(u64::try_from(byte_count).map_err(|_error| {
        PackageStoreError::InvalidSnapshot(format!(
            "blob {digest} has an invalid SQLite byte length {byte_count}"
        ))
    })?)?;
    let model = blob::Entity::find_by_id(raw_digest.to_vec())
        .one(database)
        .await?
        .ok_or(PackageStoreError::BlobNotFound(digest))?;
    let actual = BlobDigest::for_bytes(&model.bytes);
    if actual != digest {
        return Err(PackageStoreError::BlobCorrupt {
            expected: digest,
            actual,
        }
        .into());
    }
    Ok(model.bytes)
}

fn module_url(
    registry: &registry::Model, package: &str, version: &str, path: &str,
) -> HydrationResult<String> {
    let mut url = Url::parse("den-pkg://module/").map_err(|error| {
        PackageStoreError::InvalidSnapshot(format!("cannot construct package URL: {error}"))
    })?;
    {
        let mut segments = url.path_segments_mut().map_err(|()| {
            PackageStoreError::InvalidSnapshot(
                "canonical package URL cannot contain path segments".to_owned(),
            )
        })?;
        segments
            .push(&registry.kind)
            .push(&registry.base_url)
            .push(package)
            .push(version)
            .extend(path.split('/'));
    }
    Ok(url.into())
}

fn is_relative(specifier: &str) -> bool {
    matches!(specifier, "." | "..") || specifier.starts_with("./") || specifier.starts_with("../")
}

fn split_bare_specifier(specifier: &str) -> ResolutionResult<(String, String)> {
    let mut segments = specifier.split('/');
    let first = segments.next().unwrap_or_default();
    let (package, rest) = if first.starts_with('@') {
        let second = segments.next().ok_or_else(|| {
            PackageResolutionError::InvalidBareSpecifier {
                specifier: specifier.to_owned(),
            }
        })?;
        (format!("{first}/{second}"), segments.collect::<Vec<_>>())
    } else {
        (first.to_owned(), segments.collect::<Vec<_>>())
    };
    validation::package_name(&package).map_err(|_error| {
        PackageResolutionError::InvalidBareSpecifier {
            specifier: specifier.to_owned(),
        }
    })?;
    if rest.iter().any(|segment| segment.is_empty()) {
        return Err(PackageResolutionError::InvalidBareSpecifier {
            specifier: specifier.to_owned(),
        });
    }
    let export = if rest.is_empty() {
        ".".to_owned()
    } else {
        format!("./{}", rest.join("/"))
    };
    validation::export_name(&export).map_err(|_error| {
        PackageResolutionError::InvalidBareSpecifier {
            specifier: specifier.to_owned(),
        }
    })?;
    Ok((package, export))
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait as _, DbBackend, Statement};

    use super::{HydrationBudget, HydrationLimits, PackageHydrationError, PackageResolutionError};
    use crate::{
        DependencyKind, NewDependency, NewExport, NewPackageFile, NewRelease, PackageKey,
        PackageModuleSnapshot, PackageStore, PackageStoreError, RegistryId, ResolvedPackage,
        ResolvedRootEdge, RootRequirement, SolveResult, VersionId,
    };

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    const MAIN_SOURCE: &[u8] = b"export { default } from './child.js'";
    const CHILD_SOURCE: &[u8] = b"export default 42";

    #[tokio::test]
    async fn snapshot_resolves_exact_exports_and_contained_relative_files() -> TestResult {
        let (store, selected) = fixture().await?;
        let snapshot = store.hydrate_modules(&selected).await?;
        let root = snapshot.resolve("entry", "@scope/app")?;
        let child = snapshot.resolve(&root, "./child.js")?;

        assert_eq!(snapshot.package_count(), 1);
        assert_eq!(
            snapshot.module(&child).map(super::PackageModule::bytes),
            Some(b"export default 42".as_slice())
        );
        assert!(matches!(
            snapshot.resolve(&root, "../../escape.js"),
            Err(PackageResolutionError::PackageTraversal { .. })
        ));
        assert!(matches!(
            snapshot.resolve("entry", "@scope/app/private"),
            Err(PackageResolutionError::MissingExport { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn hydration_limits_accept_boundaries_and_reject_the_next_unit() -> TestResult {
        let (store, selected) = fixture().await?;
        let total_bytes = u64::try_from(MAIN_SOURCE.len())?
            .checked_add(u64::try_from(CHILD_SOURCE.len())?)
            .ok_or("fixture byte count overflowed")?;

        let mut exact = HydrationLimits::new(2, total_bytes);
        exact.max_packages = 1;
        exact.max_root_edges = 1;
        exact.max_dependency_edges = 0;
        exact.max_exports = 1;
        assert_eq!(
            store
                .hydrate_modules_with_limits(&selected, exact)
                .await?
                .module_count(),
            2
        );
        assert!(matches!(
            store
                .hydrate_modules_with_limits(&selected, HydrationLimits::new(1, total_bytes))
                .await,
            Err(PackageHydrationError::FileLimitExceeded {
                limit:     1,
                attempted: 2,
            })
        ));
        assert!(matches!(
            store
                .hydrate_modules_with_limits(
                    &selected,
                    HydrationLimits::new(2, total_bytes - 1),
                )
                .await,
            Err(PackageHydrationError::ByteLimitExceeded { limit, attempted })
                if limit == total_bytes - 1 && attempted == total_bytes
        ));

        let mut package_limited = exact;
        package_limited.max_packages = 0;
        assert!(matches!(
            store
                .hydrate_modules_with_limits(&selected, package_limited)
                .await,
            Err(PackageHydrationError::PackageLimitExceeded {
                limit:     0,
                attempted: 1,
            })
        ));
        let mut root_limited = exact;
        root_limited.max_root_edges = 0;
        assert!(matches!(
            store
                .hydrate_modules_with_limits(&selected, root_limited)
                .await,
            Err(PackageHydrationError::RootEdgeLimitExceeded {
                limit:     0,
                attempted: 1,
            })
        ));
        let mut export_limited = exact;
        export_limited.max_exports = 0;
        assert!(matches!(
            store
                .hydrate_modules_with_limits(&selected, export_limited)
                .await,
            Err(PackageHydrationError::ExportLimitExceeded {
                limit:     0,
                attempted: 1,
            })
        ));

        let defaults = HydrationLimits::default();
        assert!(defaults.max_packages > 0 && defaults.max_packages < u64::MAX);
        assert!(defaults.max_root_edges > 0 && defaults.max_root_edges < u64::MAX);
        assert!(defaults.max_dependency_edges > 0 && defaults.max_dependency_edges < u64::MAX);
        assert!(defaults.max_exports > 0 && defaults.max_exports < u64::MAX);
        assert!(defaults.max_files > 0 && defaults.max_files < u64::MAX);
        assert!(defaults.max_bytes > 0 && defaults.max_bytes < u64::MAX);
        Ok(())
    }

    #[test]
    fn hydration_budget_reports_counter_overflow() {
        let mut files = HydrationBudget::new(HydrationLimits::new(u64::MAX, u64::MAX));
        files.files = u64::MAX;
        assert!(matches!(
            files.add_files(1),
            Err(PackageHydrationError::FileCountOverflow)
        ));

        let mut bytes = HydrationBudget::new(HydrationLimits::new(u64::MAX, u64::MAX));
        bytes.bytes = u64::MAX;
        assert!(matches!(
            bytes.add_bytes(1),
            Err(PackageHydrationError::ByteCountOverflow)
        ));

        let mut dependencies = HydrationBudget::new(HydrationLimits::default());
        dependencies.dependencies = u64::MAX;
        assert!(matches!(
            dependencies.add_dependencies(1),
            Err(PackageHydrationError::DependencyCountOverflow)
        ));

        let mut exports = HydrationBudget::new(HydrationLimits::default());
        exports.exports = u64::MAX;
        assert!(matches!(
            exports.add_exports(1),
            Err(PackageHydrationError::ExportCountOverflow)
        ));
    }

    #[tokio::test]
    async fn hydration_counts_dependency_rows_before_loading_them() -> TestResult {
        let (store, selected) = fixture().await?;
        let package = selected
            .packages
            .first()
            .ok_or("fixture has no selected package")?;
        store
            .database()
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "INSERT INTO dependency(version_id, ordinal, kind, target_registry_id, \
                 package_name, requirement, alias) VALUES (?, 0, 'normal', ?, '@scope/app', '*', \
                 NULL)",
                [package.version_id.0.into(), package.registry_id.0.into()],
            ))
            .await?;
        let limits = HydrationLimits {
            max_dependency_edges: 0,
            ..HydrationLimits::default()
        };
        assert!(matches!(
            store.hydrate_modules_with_limits(&selected, limits).await,
            Err(PackageHydrationError::DependencyLimitExceeded {
                limit:     0,
                attempted: 1,
            })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn hydration_checks_blob_length_before_loading_corrupt_content() -> TestResult {
        let (store, selected) = fixture().await?;
        store
            .database()
            .execute_raw(Statement::from_string(
                DbBackend::Sqlite,
                "UPDATE blob SET bytes = X'00' WHERE digest IN (SELECT blob_digest FROM \
                 package_file)"
                    .to_owned(),
            ))
            .await?;
        assert!(matches!(
            store
                .hydrate_modules_with_limits(&selected, HydrationLimits::new(2, 0))
                .await,
            Err(PackageHydrationError::ByteLimitExceeded { limit: 0, .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn hydration_rejects_invalid_selection_and_store_content() -> TestResult {
        let (store, selected) = fixture().await?;
        let mut mismatched = selected.clone();
        mismatched
            .packages
            .first_mut()
            .ok_or("fixture has no selected package")?
            .version = "9.9.9".to_owned();
        assert!(matches!(
            store.hydrate_modules(&mismatched).await,
            Err(PackageHydrationError::SelectedVersionMismatch { .. })
        ));

        let mut duplicate = selected.clone();
        duplicate.packages.push(
            duplicate
                .packages
                .first()
                .ok_or("fixture has no selected package")?
                .clone(),
        );
        assert!(matches!(
            store.hydrate_modules(&duplicate).await,
            Err(PackageHydrationError::DuplicateSelectedPackage { .. })
        ));

        let mut missing = selected.clone();
        missing
            .packages
            .first_mut()
            .ok_or("fixture has no selected package")?
            .version_id = VersionId(i64::MAX);
        assert!(matches!(
            store.hydrate_modules(&missing).await,
            Err(PackageHydrationError::SelectedVersionNotFound(_))
        ));

        let mut incompatible_root = selected.clone();
        incompatible_root
            .roots
            .first_mut()
            .ok_or("fixture has no root edge")?
            .requirement = "^2".to_owned();
        assert!(matches!(
            store.hydrate_modules(&incompatible_root).await,
            Err(PackageHydrationError::InvalidSolutionEdge(_))
        ));

        store
            .database()
            .execute_raw(Statement::from_string(
                DbBackend::Sqlite,
                "UPDATE export SET target_path = 'missing.js'".to_owned(),
            ))
            .await?;
        assert!(matches!(
            store.hydrate_modules(&selected).await,
            Err(PackageHydrationError::DanglingExport { .. })
        ));

        store
            .database()
            .execute_raw(Statement::from_string(
                DbBackend::Sqlite,
                "UPDATE export SET target_path = 'src/main.js'".to_owned(),
            ))
            .await?;
        store
            .database()
            .execute_raw(Statement::from_string(
                DbBackend::Sqlite,
                "UPDATE blob SET bytes = X'00' WHERE digest = (SELECT blob_digest FROM \
                 package_file LIMIT 1)"
                    .to_owned(),
            ))
            .await?;
        assert!(matches!(
            store.hydrate_modules(&selected).await,
            Err(PackageHydrationError::InvalidStore(
                PackageStoreError::BlobCorrupt { .. }
            ))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn snapshot_exposes_only_roots_self_and_exact_dependencies() -> TestResult {
        let (store, app_registry, dependency_registry) = edge_fixture().await?;
        let solved = store.repository_snapshot().await?.solve(&[
            RootRequirement::new(app_registry, "app", "*"),
            RootRequirement::new(app_registry, "shared", "*"),
        ])?;
        let snapshot = store.hydrate_modules(&solved).await?;

        let app = snapshot.resolve("entry", "app")?;
        let root_shared = snapshot.resolve("entry", "shared")?;
        let dependency_shared = snapshot.resolve(&app, "shared")?;
        assert_eq!(
            module_bytes(&snapshot, &root_shared),
            Some(b"root-shared".as_slice())
        );
        assert_eq!(
            module_bytes(&snapshot, &dependency_shared),
            Some(b"dependency-shared".as_slice())
        );
        assert_ne!(root_shared, dependency_shared);
        assert_eq!(snapshot.resolve(&app, "app")?, app);

        let secret = snapshot.resolve(&dependency_shared, "secret")?;
        assert_eq!(module_bytes(&snapshot, &secret), Some(b"secret".as_slice()));
        assert!(matches!(
            snapshot.resolve_if_claimed(&app, "secret"),
            Some(Err(PackageResolutionError::UndeclaredDependency { .. }))
        ));
        assert!(snapshot.resolve_if_claimed("entry", "secret").is_none());
        assert!(snapshot.resolve_if_claimed("entry", "local-file").is_none());
        assert!(snapshot.resolve_if_claimed("entry", "@invalid").is_none());
        assert!(matches!(
            snapshot.resolve_if_claimed(&app, "@invalid"),
            Some(Err(PackageResolutionError::InvalidBareSpecifier { .. }))
        ));
        for specifier in ["https://example.invalid/escape.js", "/tmp/escape.js"] {
            assert!(matches!(
                snapshot.resolve_if_claimed(&app, specifier),
                Some(Err(PackageResolutionError::UnsupportedSpecifier { .. }))
            ));
        }
        assert!(matches!(
            snapshot.resolve_if_claimed("entry", &secret),
            Some(Err(PackageResolutionError::DirectCanonicalImport { .. }))
        ));

        let dependency_edge = solved
            .dependencies
            .iter()
            .find(|edge| edge.target.registry_id == dependency_registry)
            .ok_or("missing cross-registry dependency edge")?;
        assert_eq!(dependency_edge.importer.name, "app");
        assert_eq!(dependency_edge.target.name, "shared");
        Ok(())
    }

    #[tokio::test]
    async fn hydration_rejects_dependency_edges_that_disagree_with_metadata() -> TestResult {
        let (store, app_registry, dependency_registry) = edge_fixture().await?;
        let solved = store
            .repository_snapshot()
            .await?
            .solve(&[RootRequirement::new(app_registry, "app", "*")])?;
        let mut missing_edge = solved.clone();
        missing_edge.dependencies.clear();
        assert!(matches!(
            store.hydrate_modules(&missing_edge).await,
            Err(PackageHydrationError::InvalidSolutionEdge(_))
        ));

        let incompatible = store
            .package(dependency_registry, "shared")
            .await?
            .ok_or("missing dependency package")?
            .versions
            .into_iter()
            .find(|version| version.version == "2.0.0")
            .ok_or("missing incompatible dependency version")?;
        let mut incompatible_selection = solved;
        let selected = incompatible_selection
            .packages
            .iter_mut()
            .find(|package| {
                package.registry_id == dependency_registry && package.package == "shared"
            })
            .ok_or("missing selected dependency package")?;
        selected.version_id = incompatible.id;
        selected.version = incompatible.version;
        let edge = incompatible_selection
            .dependencies
            .iter_mut()
            .find(|edge| {
                edge.target.registry_id == dependency_registry && edge.target.name == "shared"
            })
            .ok_or("missing dependency edge")?;
        edge.target_version_id = incompatible.id;
        assert!(matches!(
            store.hydrate_modules(&incompatible_selection).await,
            Err(PackageHydrationError::InvalidSolutionEdge(_))
        ));
        Ok(())
    }

    fn module_bytes<'a>(snapshot: &'a PackageModuleSnapshot, url: &str) -> Option<&'a [u8]> {
        snapshot.module(url).map(super::PackageModule::bytes)
    }

    async fn edge_fixture() -> TestResult<(PackageStore, RegistryId, RegistryId)> {
        let store = PackageStore::open_in_memory().await?;
        let app_registry = store.add_registry("npm", "https://app.example/").await?;
        let dependency_registry = store
            .add_registry("npm", "https://dependencies.example/")
            .await?;
        insert_module_release(&store, app_registry, "shared", "1.0.0", b"root-shared", &[]).await?;
        insert_module_release(
            &store,
            dependency_registry,
            "secret",
            "1.0.0",
            b"secret",
            &[],
        )
        .await?;
        insert_module_release(
            &store,
            dependency_registry,
            "shared",
            "1.0.0",
            b"dependency-shared",
            &[(dependency_registry, "secret", "^1")],
        )
        .await?;
        insert_module_release(
            &store,
            dependency_registry,
            "shared",
            "2.0.0",
            b"incompatible-shared",
            &[],
        )
        .await?;
        insert_module_release(&store, app_registry, "app", "1.0.0", b"app", &[(
            dependency_registry,
            "shared",
            "^1",
        )])
        .await?;
        Ok((store, app_registry, dependency_registry))
    }

    async fn insert_module_release(
        store: &PackageStore, registry: RegistryId, package: &str, version: &str, source: &[u8],
        dependencies: &[(RegistryId, &str, &str)],
    ) -> TestResult<VersionId> {
        let blob = store.insert_blob(source).await?;
        let mut release = NewRelease::new(registry, package, version);
        release.exports.push(NewExport {
            name:   ".".to_owned(),
            target: "main.js".to_owned(),
        });
        release.files.push(NewPackageFile {
            path: "main.js".to_owned(),
            blob,
            media_type: Some("text/javascript".to_owned()),
            mode: 0o644,
        });
        release.dependencies = dependencies
            .iter()
            .map(|(registry, dependency, requirement)| {
                NewDependency {
                    kind:               DependencyKind::Normal,
                    target_registry_id: Some(*registry),
                    package:            (*dependency).to_owned(),
                    requirement:        (*requirement).to_owned(),
                    alias:              None,
                }
            })
            .collect();
        Ok(store.insert_release(&release).await?)
    }

    async fn fixture() -> TestResult<(PackageStore, SolveResult)> {
        let store = PackageStore::open_in_memory().await?;
        let registry = store.add_registry("jsr", "https://jsr.example/").await?;
        let main = store.insert_blob(MAIN_SOURCE).await?;
        let child = store.insert_blob(CHILD_SOURCE).await?;
        let mut release = NewRelease::new(registry, "@scope/app", "1.0.0");
        release.exports.push(NewExport {
            name:   ".".to_owned(),
            target: "src/main.js".to_owned(),
        });
        release.files.extend([
            NewPackageFile {
                path:       "src/main.js".to_owned(),
                blob:       main,
                media_type: Some("text/javascript".to_owned()),
                mode:       0o644,
            },
            NewPackageFile {
                path:       "src/child.js".to_owned(),
                blob:       child,
                media_type: Some("text/javascript".to_owned()),
                mode:       0o644,
            },
        ]);
        let version_id = store.insert_release(&release).await?;
        Ok((store, SolveResult {
            packages:     vec![ResolvedPackage {
                registry_id: registry,
                package: "@scope/app".to_owned(),
                version_id,
                version: "1.0.0".to_owned(),
            }],
            roots:        vec![ResolvedRootEdge {
                specifier:         "@scope/app".to_owned(),
                requirement:       "*".to_owned(),
                target:            PackageKey {
                    registry_id: registry,
                    name:        "@scope/app".to_owned(),
                },
                target_version_id: version_id,
            }],
            dependencies: Vec::new(),
        }))
    }
}
