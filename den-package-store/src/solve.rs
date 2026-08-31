use std::{
    any::Any,
    collections::BTreeMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use resolvo::{
    Candidates, Condition, ConditionId, ConditionalRequirement, Dependencies, DependencyProvider,
    Interner, KnownDependencies, NameId, Problem, SolvableId, Solver, SolverCache, StringId,
    UnsolvableOrCancelled, VersionSetId, VersionSetUnionId,
    utils::{Pool, VersionSet},
};

use crate::{
    DependencyKind, PackageKey, PackageStoreError, RepositorySnapshot, ResolvedDependencyEdge,
    ResolvedPackage, ResolvedRootEdge, Result, SnapshotDependency, SolveResult, VersionId,
    validation,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootRequirement {
    pub specifier:   String,
    pub registry_id: crate::RegistryId,
    pub package:     String,
    pub requirement: String,
}

impl RootRequirement {
    #[must_use]
    pub fn new<P: Into<String>, R: Into<String>>(
        registry_id: crate::RegistryId, package: P, requirement: R,
    ) -> Self {
        let package = package.into();
        Self {
            specifier: package.clone(),
            registry_id,
            package,
            requirement: requirement.into(),
        }
    }

    #[must_use]
    pub fn aliased<S: Into<String>, P: Into<String>, R: Into<String>>(
        specifier: S, registry_id: crate::RegistryId, package: P, requirement: R,
    ) -> Self {
        Self {
            specifier: specifier.into(),
            registry_id,
            package: package.into(),
            requirement: requirement.into(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) { self.0.store(true, Ordering::Release); }

    #[must_use]
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::Acquire) }
}

impl RepositorySnapshot {
    /// Solve one flat version per `(registry, package)` key.
    ///
    /// Releases with optional, peer, or aliased dependencies are excluded
    /// until scoped package instances can model their semantics. This API must
    /// not be used as if it already models an npm-style nested tree.
    pub fn solve(&self, roots: &[RootRequirement]) -> Result<SolveResult> {
        self.solve_with_cancellation(roots, None)
    }

    pub fn solve_with_cancellation(
        &self, roots: &[RootRequirement], cancellation: Option<CancellationToken>,
    ) -> Result<SolveResult> {
        if cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(PackageStoreError::Cancelled);
        }
        let provider = SnapshotProvider::new(self, cancellation)?;
        let requirements = roots
            .iter()
            .map(|root| provider.root_requirement(root))
            .collect::<Result<Vec<_>>>()?;
        let mut solver = Solver::new(provider);
        let selected_ids = match solver.solve(Problem::new().requirements(requirements)) {
            Ok(selected_ids) => selected_ids,
            Err(UnsolvableOrCancelled::Unsolvable(conflict)) => {
                return Err(PackageStoreError::Conflict(
                    conflict.display_user_friendly(&solver).to_string(),
                ));
            }
            Err(UnsolvableOrCancelled::Cancelled(_)) => return Err(PackageStoreError::Cancelled),
        };

        let provider = solver.provider();
        let mut packages = selected_ids
            .into_iter()
            .map(|solvable_id| provider.resolved_package(solvable_id))
            .collect::<Vec<_>>();
        packages.sort_by(|left, right| {
            left.registry_id
                .cmp(&right.registry_id)
                .then_with(|| left.package.cmp(&right.package))
                .then_with(|| left.version.cmp(&right.version))
                .then_with(|| left.version_id.cmp(&right.version_id))
        });
        let selected = packages
            .iter()
            .map(|package| {
                (
                    PackageKey {
                        registry_id: package.registry_id,
                        name:        package.package.clone(),
                    },
                    package,
                )
            })
            .collect::<BTreeMap<_, _>>();

        let mut root_edges = roots
            .iter()
            .map(|root| {
                let target = PackageKey {
                    registry_id: root.registry_id,
                    name:        root.package.clone(),
                };
                let selected = selected.get(&target).ok_or_else(|| {
                    PackageStoreError::InvalidSnapshot(format!(
                        "solver omitted selected root `{target}`"
                    ))
                })?;
                Ok(ResolvedRootEdge {
                    specifier: root.specifier.clone(),
                    requirement: root.requirement.clone(),
                    target,
                    target_version_id: selected.version_id,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        root_edges.sort();
        root_edges.dedup();

        let mut dependency_edges = Vec::new();
        for importer in &packages {
            let importer_key = PackageKey {
                registry_id: importer.registry_id,
                name:        importer.package.clone(),
            };
            for dependency in provider
                .dependencies
                .get(&importer.version_id)
                .into_iter()
                .flatten()
            {
                if dependency.kind != DependencyKind::Normal || dependency.alias.is_some() {
                    return Err(PackageStoreError::InvalidSnapshot(format!(
                        "solver selected unsupported dependency edge from `{importer_key}`"
                    )));
                }
                let target = selected.get(&dependency.package_key).ok_or_else(|| {
                    PackageStoreError::InvalidSnapshot(format!(
                        "solver omitted dependency `{}` required by `{importer_key}`",
                        dependency.package_key
                    ))
                })?;
                dependency_edges.push(ResolvedDependencyEdge {
                    importer:            importer_key.clone(),
                    importer_version_id: importer.version_id,
                    specifier:           dependency.package_key.name.clone(),
                    requirement:         dependency.requirement.clone(),
                    target:              dependency.package_key.clone(),
                    target_version_id:   target.version_id,
                });
            }
        }
        dependency_edges.sort();
        dependency_edges.dedup();

        Ok(SolveResult {
            packages,
            roots: root_edges,
            dependencies: dependency_edges,
        })
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct JsRange {
    raw:    String,
    parsed: node_semver::Range,
}

impl fmt::Display for JsRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.raw.fmt(f) }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CandidateVersion {
    version_id:       VersionId,
    raw:              String,
    parsed:           node_semver::Version,
    exclusion_reason: Option<String>,
}

impl fmt::Display for CandidateVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.raw.fmt(f) }
}

impl VersionSet for JsRange {
    type V = CandidateVersion;
}

struct SnapshotProvider {
    pool:         Pool<JsRange, PackageKey>,
    candidates:   BTreeMap<PackageKey, Vec<SolvableId>>,
    dependencies: BTreeMap<VersionId, Vec<SnapshotDependency>>,
    cancellation: Option<CancellationToken>,
}

impl SnapshotProvider {
    fn new(snapshot: &RepositorySnapshot, cancellation: Option<CancellationToken>) -> Result<Self> {
        let pool = Pool::new();
        let mut candidates = BTreeMap::new();
        let mut dependencies = BTreeMap::new();

        for (package_key, versions) in &snapshot.packages {
            if cancellation
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                return Err(PackageStoreError::Cancelled);
            }
            let name_id = pool.intern_package_name(package_key.clone());
            let mut versions = versions.clone();
            versions.sort_by(|left, right| {
                left.parsed_version
                    .cmp(&right.parsed_version)
                    .then_with(|| left.raw_version.cmp(&right.raw_version))
                    .then_with(|| left.id.cmp(&right.id))
            });
            let mut package_candidates = Vec::with_capacity(versions.len());
            for version in versions {
                if cancellation
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
                {
                    return Err(PackageStoreError::Cancelled);
                }
                let unsupported = version
                    .dependencies
                    .iter()
                    .find(|dependency| {
                        dependency.kind != DependencyKind::Normal || dependency.alias.is_some()
                    })
                    .map(|dependency| {
                        dependency.alias.as_ref().map_or_else(
                            || {
                                format!(
                                    "dependency kind `{}` requires scoped/optional solving",
                                    dependency.kind.as_str()
                                )
                            },
                            |alias| {
                                format!(
                                    "dependency alias `{alias}` is unsupported by flat package \
                                     identities"
                                )
                            },
                        )
                    });
                let exclusion_reason = match (&version.yanked_reason, unsupported) {
                    (Some(yanked), Some(unsupported)) => {
                        Some(format!("yanked: {yanked}; {unsupported}"))
                    }
                    (Some(yanked), None) => Some(format!("yanked: {yanked}")),
                    (None, unsupported) => unsupported,
                };
                let solvable = pool.intern_solvable(name_id, CandidateVersion {
                    version_id: version.id,
                    raw: version.raw_version,
                    parsed: version.parsed_version,
                    exclusion_reason,
                });
                package_candidates.push(solvable);
                dependencies.insert(version.id, version.dependencies);
            }
            candidates.insert(package_key.clone(), package_candidates);
        }

        Ok(Self {
            pool,
            candidates,
            dependencies,
            cancellation,
        })
    }

    fn root_requirement(&self, root: &RootRequirement) -> Result<ConditionalRequirement> {
        validation::package_name(&root.package)?;
        let parsed = node_semver::Range::parse(&root.requirement).map_err(|error| {
            PackageStoreError::InvalidVersionRange {
                range:  root.requirement.clone(),
                reason: error.to_string(),
            }
        })?;
        let name_id = self.pool.intern_package_name(PackageKey {
            registry_id: root.registry_id,
            name:        root.package.clone(),
        });
        Ok(self
            .pool
            .intern_version_set(name_id, JsRange {
                raw: root.requirement.clone(),
                parsed,
            })
            .into())
    }

    fn resolved_package(&self, solvable_id: SolvableId) -> ResolvedPackage {
        let solvable = self.pool.resolve_solvable(solvable_id);
        let package = self.pool.resolve_package_name(solvable.name);
        ResolvedPackage {
            registry_id: package.registry_id,
            package:     package.name.clone(),
            version_id:  solvable.record.version_id,
            version:     solvable.record.raw.clone(),
        }
    }

    fn dependency_version_set(&self, dependency: &SnapshotDependency) -> VersionSetId {
        let name_id = self
            .pool
            .intern_package_name(dependency.package_key.clone());
        self.pool.intern_version_set(name_id, JsRange {
            raw:    dependency.requirement.clone(),
            parsed: dependency.parsed_requirement.clone(),
        })
    }
}

impl Interner for SnapshotProvider {
    type NameId = NameId;
    type SolvableId = SolvableId;

    fn display_solvable(&self, solvable: SolvableId) -> impl fmt::Display + '_ {
        let solvable = self.pool.resolve_solvable(solvable);
        format!(
            "{}@{}",
            self.pool.resolve_package_name(solvable.name),
            solvable.record.raw
        )
    }

    fn display_name(&self, name: NameId) -> impl fmt::Display + '_ {
        self.pool.resolve_package_name(name).clone()
    }

    fn display_version_set(&self, version_set: VersionSetId) -> impl fmt::Display + '_ {
        self.pool.resolve_version_set(version_set).clone()
    }

    fn display_string(&self, string_id: StringId) -> impl fmt::Display + '_ {
        self.pool.resolve_string(string_id).to_owned()
    }

    fn version_set_name(&self, version_set: VersionSetId) -> NameId {
        self.pool.resolve_version_set_package_name(version_set)
    }

    fn solvable_name(&self, solvable: SolvableId) -> NameId {
        self.pool.resolve_solvable(solvable).name
    }

    fn version_sets_in_union(
        &self, version_set_union: VersionSetUnionId,
    ) -> impl Iterator<Item = VersionSetId> {
        self.pool.resolve_version_set_union(version_set_union)
    }

    fn resolve_condition(&self, condition: ConditionId) -> Condition {
        self.pool.resolve_condition(condition).clone()
    }
}

impl DependencyProvider for SnapshotProvider {
    async fn filter_candidates(
        &self, candidates: &[SolvableId], version_set: VersionSetId, inverse: bool,
    ) -> Vec<SolvableId> {
        let range = self.pool.resolve_version_set(version_set);
        candidates
            .iter()
            .copied()
            .filter(|candidate| {
                let matches = self
                    .pool
                    .resolve_solvable(*candidate)
                    .record
                    .parsed
                    .satisfies(&range.parsed);
                matches != inverse
            })
            .collect()
    }

    async fn get_candidates(&self, name: NameId) -> Option<Candidates> {
        let package = self.pool.resolve_package_name(name);
        let candidates = self.candidates.get(package)?;
        let excluded = candidates
            .iter()
            .filter_map(|candidate| {
                self.pool
                    .resolve_solvable(*candidate)
                    .record
                    .exclusion_reason
                    .as_ref()
                    .map(|reason| (*candidate, self.pool.intern_string(reason)))
            })
            .collect();
        Some(Candidates {
            candidates: candidates.clone(),
            excluded,
            ..Candidates::default()
        })
    }

    async fn sort_candidates(&self, _solver: &SolverCache<Self>, solvables: &mut [SolvableId]) {
        solvables.sort_by(|left, right| {
            let left = &self.pool.resolve_solvable(*left).record;
            let right = &self.pool.resolve_solvable(*right).record;
            right
                .parsed
                .cmp(&left.parsed)
                .then_with(|| left.raw.cmp(&right.raw))
                .then_with(|| left.version_id.cmp(&right.version_id))
        });
    }

    async fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        let version_id = self.pool.resolve_solvable(solvable).record.version_id;
        let Some(dependencies) = self.dependencies.get(&version_id) else {
            return Dependencies::Known(KnownDependencies::default());
        };
        let mut known = KnownDependencies::default();
        for dependency in dependencies {
            let version_set = self.dependency_version_set(dependency);
            if dependency.kind == DependencyKind::Normal {
                known.requirements.push(version_set.into());
            } else {
                return Dependencies::Unknown(self.pool.intern_string(format!(
                    "unsupported dependency kind `{}`",
                    dependency.kind.as_str()
                )));
            }
        }
        Dependencies::Known(known)
    }

    fn should_cancel_with_value(&self) -> Option<Box<dyn Any>> {
        self.cancellation
            .as_ref()
            .filter(|token| token.is_cancelled())
            .map(|_| Box::new(()) as Box<dyn Any>)
    }
}
