use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
};

use den_capabilities::{
    Capability, ImportScope, NameScope, NetworkScope, NormalizedPath, Policy, PortRange, Rule,
    Scope, ScopeError,
};
use jsonc_parser::{ParseOptions, errors::ParseError, parse_to_serde_value};
use serde::Deserialize;
use thiserror::Error;
use url::Url;

pub const CONFIG_FILE_NAMES: [&str; 2] = ["den.json", "den.jsonc"];

pub type Result<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to determine the current directory: {0}")]
    CurrentDirectory(#[source] io::Error),
    #[error("configuration file `{path}` does not exist")]
    NotFound { path: PathBuf },
    #[error("configuration path `{path}` is not a regular file")]
    NotFile { path: PathBuf },
    #[error("failed to read configuration file `{path}`: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to parse configuration file `{path}`: {source}")]
    Parse { path: PathBuf, source: ParseError },
    #[error("configuration file `{path}` contains an empty {capability} permission path")]
    EmptyPermissionPath {
        path:       PathBuf,
        capability: &'static str,
    },
    #[error("cannot represent configuration path `{path}` as a file URL")]
    InvalidFileUrl { path: PathBuf },
}

#[derive(Debug, Error)]
#[error("invalid {capability} permission `{value}`: {source}")]
pub struct PolicyError {
    capability: Capability,
    value:      String,
    #[source]
    source:     ScopeError,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    imports:          Option<BTreeMap<String, String>>,
    import_map:       Option<PathBuf>,
    package_store:    Option<PathBuf>,
    registries:       Option<BTreeMap<String, RegistryConfig>>,
    dependencies:     Option<BTreeMap<String, String>>,
    dev_dependencies: Option<BTreeMap<String, String>>,
    permissions:      Option<PermissionsConfig>,
    budgets:          Option<BudgetsConfig>,
    tasks:            Option<BTreeMap<String, TaskConfig>>,
    workspace:        Option<WorkspaceConfig>,
    env_files:        Option<Vec<PathBuf>>,
    preloads:         Option<Vec<PathBuf>>,
    offline:          Option<bool>,
    frozen:           Option<bool>,
    reload:           Option<bool>,
    source:           Option<ConfigSource>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct RawConfig {
    imports:          Option<BTreeMap<String, String>>,
    import_map:       Option<PathBuf>,
    package_store:    Option<PathBuf>,
    registries:       Option<BTreeMap<String, RegistryConfig>>,
    dependencies:     Option<BTreeMap<String, String>>,
    dev_dependencies: Option<BTreeMap<String, String>>,
    permissions:      Option<PermissionsConfig>,
    budgets:          Option<BudgetsConfig>,
    tasks:            Option<BTreeMap<String, TaskConfig>>,
    workspace:        Option<WorkspaceConfig>,
    env_files:        Option<Vec<PathBuf>>,
    preloads:         Option<Vec<PathBuf>>,
    offline:          Option<bool>,
    frozen:           Option<bool>,
    reload:           Option<bool>,
}

impl Default for RawConfig {
    fn default() -> Self { Self::from(Config::default()) }
}

impl From<Config> for RawConfig {
    fn from(config: Config) -> Self {
        Self {
            imports:          config.imports,
            import_map:       config.import_map,
            package_store:    config.package_store,
            registries:       config.registries,
            dependencies:     config.dependencies,
            dev_dependencies: config.dev_dependencies,
            permissions:      config.permissions,
            budgets:          config.budgets,
            tasks:            config.tasks,
            workspace:        config.workspace,
            env_files:        config.env_files,
            preloads:         config.preloads,
            offline:          config.offline,
            frozen:           config.frozen,
            reload:           config.reload,
        }
    }
}

impl From<RawConfig> for Config {
    fn from(raw: RawConfig) -> Self {
        Self {
            imports:          raw.imports,
            import_map:       raw.import_map,
            package_store:    raw.package_store,
            registries:       raw.registries,
            dependencies:     raw.dependencies,
            dev_dependencies: raw.dev_dependencies,
            permissions:      raw.permissions,
            budgets:          raw.budgets,
            tasks:            raw.tasks,
            workspace:        raw.workspace,
            env_files:        raw.env_files,
            preloads:         raw.preloads,
            offline:          raw.offline,
            frozen:           raw.frozen,
            reload:           raw.reload,
            source:           None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigSource {
    pub path: PathBuf,
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum RegistryConfig {
    Url(String),
    Detailed(RegistryOptions),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegistryOptions {
    pub url:       String,
    pub token_env: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Access<T> {
    All(bool),
    List(Vec<T>),
    Rules(AccessRules<T>),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccessRules<T> {
    pub allow: Option<AccessValue<T>>,
    pub deny:  Option<AccessValue<T>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum AccessValue<T> {
    All(bool),
    List(Vec<T>),
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct PermissionsConfig {
    pub read:        Option<Access<PathBuf>>,
    pub write:       Option<Access<PathBuf>>,
    pub net_connect: Option<Access<String>>,
    pub net_listen:  Option<Access<String>>,
    pub env:         Option<Access<String>>,
    pub run:         Option<Access<PathBuf>>,
    pub sys:         Option<Access<String>>,
    pub ffi:         Option<Access<PathBuf>>,
    pub imports:     Option<Access<String>>,
    pub secrets:     Option<Access<String>>,
    pub prompt:      Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct BudgetsConfig {
    pub heap_bytes:  Option<u64>,
    pub stack_bytes: Option<usize>,
    pub timeout_ms:  Option<u64>,
    pub max_workers: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum TaskConfig {
    Command(String),
    Detailed(TaskOptions),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskOptions {
    pub command:      String,
    pub description:  Option<String>,
    pub cwd:          Option<PathBuf>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub env:          BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum WorkspaceConfig {
    Members(Vec<PathBuf>),
    Detailed(WorkspaceOptions),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceOptions {
    pub members: Vec<PathBuf>,
    #[serde(default)]
    pub exclude: Vec<PathBuf>,
}

impl Config {
    pub const fn source(&self) -> Option<&ConfigSource> { self.source.as_ref() }

    pub fn root(&self) -> Option<&Path> { self.source.as_ref().map(|source| source.root.as_path()) }

    pub const fn imports(&self) -> Option<&BTreeMap<String, String>> { self.imports.as_ref() }

    pub fn import_map(&self) -> Option<&Path> { self.import_map.as_deref() }

    pub fn package_store(&self) -> Option<&Path> { self.package_store.as_deref() }

    pub const fn registries(&self) -> Option<&BTreeMap<String, RegistryConfig>> {
        self.registries.as_ref()
    }

    pub const fn dependencies(&self) -> Option<&BTreeMap<String, String>> {
        self.dependencies.as_ref()
    }

    pub const fn dev_dependencies(&self) -> Option<&BTreeMap<String, String>> {
        self.dev_dependencies.as_ref()
    }

    pub const fn permissions(&self) -> Option<&PermissionsConfig> { self.permissions.as_ref() }

    /// Convert configured permissions to den's deny-by-default host policy.
    pub fn policy(&self) -> std::result::Result<Policy, PolicyError> {
        self.permissions
            .as_ref()
            .map_or_else(|| Ok(Policy::default()), PermissionsConfig::policy)
    }

    pub const fn budgets(&self) -> Option<&BudgetsConfig> { self.budgets.as_ref() }

    pub const fn tasks(&self) -> Option<&BTreeMap<String, TaskConfig>> { self.tasks.as_ref() }

    pub const fn workspace(&self) -> Option<&WorkspaceConfig> { self.workspace.as_ref() }

    pub fn env_files(&self) -> Option<&[PathBuf]> { self.env_files.as_deref() }

    pub fn preloads(&self) -> Option<&[PathBuf]> { self.preloads.as_deref() }

    pub const fn offline(&self) -> Option<bool> { self.offline }

    pub const fn frozen(&self) -> Option<bool> { self.frozen }

    pub const fn reload(&self) -> Option<bool> { self.reload }

    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = absolute(path.as_ref())?;
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(ConfigError::NotFile { path }),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(ConfigError::NotFound { path });
            }
            Err(source) => {
                return Err(ConfigError::Read { path, source });
            }
        }

        let text = fs::read_to_string(&path).map_err(|source| {
            ConfigError::Read {
                path: path.clone(),
                source,
            }
        })?;
        let raw: RawConfig = parse_to_serde_value(&text, &jsonc_options()).map_err(|source| {
            ConfigError::Parse {
                path: path.clone(),
                source,
            }
        })?;
        let mut config = Self::from(raw);
        let root = path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        config.resolve_paths(&root, &path)?;
        config.source = Some(ConfigSource { path, root });
        Ok(config)
    }

    pub fn discover<P: AsRef<Path>>(start: P, explicit: Option<&Path>) -> Result<Option<Self>> {
        if let Some(path) = explicit {
            return Self::load(path).map(Some);
        }

        let start = absolute(start.as_ref())?;
        let directory = if start.is_file() {
            start.parent().unwrap_or(&start)
        } else {
            start.as_path()
        };

        for directory in directory.ancestors() {
            for name in CONFIG_FILE_NAMES {
                let candidate = directory.join(name);
                match fs::metadata(&candidate) {
                    Ok(metadata) if metadata.is_file() => return Self::load(candidate).map(Some),
                    Ok(_) => return Err(ConfigError::NotFile { path: candidate }),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(ConfigError::Read {
                            path: candidate,
                            source,
                        });
                    }
                }
            }
        }
        Ok(None)
    }

    /// Merge an authoritative higher-priority layer into this one. Maps and
    /// nested settings merge by key; lists and scalar values replace only when
    /// explicitly set.
    ///
    /// This overlay may broaden permissions and is intended for CLI/user
    /// precedence only. Workspace and worker policies must be converted into
    /// separate capability policies and intersected instead.
    pub fn merge(&mut self, higher: Self) {
        merge_map(&mut self.imports, higher.imports);
        replace(&mut self.import_map, higher.import_map);
        replace(&mut self.package_store, higher.package_store);
        merge_map(&mut self.registries, higher.registries);
        merge_map(&mut self.dependencies, higher.dependencies);
        merge_map(&mut self.dev_dependencies, higher.dev_dependencies);
        merge_nested(
            &mut self.permissions,
            higher.permissions,
            PermissionsConfig::merge,
        );
        merge_nested(&mut self.budgets, higher.budgets, BudgetsConfig::merge);
        merge_map(&mut self.tasks, higher.tasks);
        replace(&mut self.workspace, higher.workspace);
        replace(&mut self.env_files, higher.env_files);
        replace(&mut self.preloads, higher.preloads);
        replace(&mut self.offline, higher.offline);
        replace(&mut self.frozen, higher.frozen);
        replace(&mut self.reload, higher.reload);
        replace(&mut self.source, higher.source);
    }

    #[must_use]
    pub fn merged(mut self, higher: Self) -> Self {
        self.merge(higher);
        self
    }

    fn resolve_paths(&mut self, root: &Path, source_path: &Path) -> Result<()> {
        resolve_import_targets(root, &mut self.imports)?;
        resolve_dependency_targets(root, &mut self.dependencies)?;
        resolve_dependency_targets(root, &mut self.dev_dependencies)?;
        resolve_optional_path(root, &mut self.import_map);
        resolve_optional_path(root, &mut self.package_store);
        resolve_paths(root, &mut self.env_files);
        resolve_paths(root, &mut self.preloads);

        if let Some(permissions) = &mut self.permissions {
            permissions.resolve_paths(root, source_path)?;
        }
        if let Some(tasks) = &mut self.tasks {
            for task in tasks.values_mut() {
                task.resolve_paths(root);
            }
        }
        if let Some(workspace) = &mut self.workspace {
            workspace.resolve_paths(root);
        }
        Ok(())
    }
}

impl PermissionsConfig {
    pub fn policy(&self) -> std::result::Result<Policy, PolicyError> {
        let mut rules = Vec::new();
        append_access(&mut rules, Capability::Read, self.read.as_ref(), |value| {
            NormalizedPath::new(value).map(Scope::Read)
        })?;
        append_access(
            &mut rules,
            Capability::Write,
            self.write.as_ref(),
            |value| NormalizedPath::new(value).map(Scope::Write),
        )?;
        append_access(
            &mut rules,
            Capability::NetConnect,
            self.net_connect.as_ref(),
            |value| network_scope(value, Capability::NetConnect),
        )?;
        append_access(
            &mut rules,
            Capability::NetListen,
            self.net_listen.as_ref(),
            |value| network_scope(value, Capability::NetListen),
        )?;
        append_access(&mut rules, Capability::Env, self.env.as_ref(), |value| {
            NameScope::exact(value).map(Scope::Env)
        })?;
        append_access(&mut rules, Capability::Run, self.run.as_ref(), |value| {
            NormalizedPath::new(value).map(Scope::Run)
        })?;
        append_access(&mut rules, Capability::Sys, self.sys.as_ref(), |value| {
            NameScope::exact(value).map(Scope::Sys)
        })?;
        append_access(&mut rules, Capability::Ffi, self.ffi.as_ref(), |value| {
            NormalizedPath::new(value).map(Scope::Ffi)
        })?;
        append_access(
            &mut rules,
            Capability::Import,
            self.imports.as_ref(),
            |value| {
                if value.ends_with('/') {
                    ImportScope::prefix(value).map(Scope::Import)
                } else {
                    ImportScope::exact(value).map(Scope::Import)
                }
            },
        )?;
        append_access(
            &mut rules,
            Capability::Secrets,
            self.secrets.as_ref(),
            |value| NameScope::exact(value).map(Scope::Secrets),
        )?;
        Ok(Policy::new(rules))
    }

    fn merge(&mut self, higher: Self) {
        replace(&mut self.read, higher.read);
        replace(&mut self.write, higher.write);
        replace(&mut self.net_connect, higher.net_connect);
        replace(&mut self.net_listen, higher.net_listen);
        replace(&mut self.env, higher.env);
        replace(&mut self.run, higher.run);
        replace(&mut self.sys, higher.sys);
        replace(&mut self.ffi, higher.ffi);
        replace(&mut self.imports, higher.imports);
        replace(&mut self.secrets, higher.secrets);
        replace(&mut self.prompt, higher.prompt);
    }

    fn resolve_paths(&mut self, root: &Path, source_path: &Path) -> Result<()> {
        resolve_access_paths(root, source_path, "read", &mut self.read)?;
        resolve_access_paths(root, source_path, "write", &mut self.write)?;
        resolve_access_paths(root, source_path, "run", &mut self.run)?;
        resolve_access_paths(root, source_path, "ffi", &mut self.ffi)
    }
}

fn append_access<T>(
    rules: &mut Vec<Rule>, capability: Capability, access: Option<&Access<T>>,
    scope: impl Fn(&T) -> std::result::Result<Scope, ScopeError>,
) -> std::result::Result<(), PolicyError>
where
    T: std::fmt::Debug,
{
    let Some(access) = access else { return Ok(()) };
    match access {
        Access::All(allowed) => {
            rules.push(if *allowed {
                Rule::allow(Scope::All(capability))
            } else {
                Rule::deny(Scope::All(capability))
            });
        }
        Access::List(values) => {
            append_scoped_rules(rules, capability, values, Rule::allow, &scope)?
        }
        Access::Rules(access) => {
            if let Some(allow) = &access.allow {
                append_access_value(rules, capability, allow, Rule::allow, &scope)?;
            }
            if let Some(deny) = &access.deny {
                append_access_value(rules, capability, deny, Rule::deny, &scope)?;
            }
        }
    }
    Ok(())
}

fn append_access_value<T>(
    rules: &mut Vec<Rule>, capability: Capability, access: &AccessValue<T>,
    rule: fn(Scope) -> Rule, scope: &impl Fn(&T) -> std::result::Result<Scope, ScopeError>,
) -> std::result::Result<(), PolicyError>
where
    T: std::fmt::Debug,
{
    match access {
        AccessValue::All(true) => rules.push(rule(Scope::All(capability))),
        AccessValue::All(false) => {}
        AccessValue::List(values) => append_scoped_rules(rules, capability, values, rule, scope)?,
    }
    Ok(())
}

fn append_scoped_rules<T>(
    rules: &mut Vec<Rule>, capability: Capability, values: &[T], rule: fn(Scope) -> Rule,
    scope: &impl Fn(&T) -> std::result::Result<Scope, ScopeError>,
) -> std::result::Result<(), PolicyError>
where
    T: std::fmt::Debug,
{
    for value in values {
        rules.push(rule(scope(value).map_err(|source| {
            PolicyError {
                capability,
                value: format!("{value:?}"),
                source,
            }
        })?));
    }
    Ok(())
}

fn network_scope(value: &str, capability: Capability) -> std::result::Result<Scope, ScopeError> {
    let explicit_port = value
        .strip_prefix('[')
        .and_then(|value| value.split_once("]:"))
        .or_else(|| {
            (value.matches(':').count() == 1)
                .then(|| value.split_once(':'))
                .flatten()
        });
    let (host, ports) = explicit_port.map_or_else(
        || Ok((value, PortRange::new(0, u16::MAX)?)),
        |(host, port)| {
            let port = port
                .parse::<u16>()
                .map_err(|_error| ScopeError::InvalidPortRange)?;
            Ok((host, PortRange::exact(port)))
        },
    )?;
    let network = if host.contains('/') {
        NetworkScope::cidr(host, ports)?
    } else {
        NetworkScope::host(host.trim_matches(['[', ']']), ports)?
    };
    Ok(match capability {
        Capability::NetConnect => Scope::NetConnect(network),
        Capability::NetListen => Scope::NetListen(network),
        _ => return Err(ScopeError::InvalidHost),
    })
}

impl BudgetsConfig {
    fn merge(&mut self, higher: Self) {
        replace(&mut self.heap_bytes, higher.heap_bytes);
        replace(&mut self.stack_bytes, higher.stack_bytes);
        replace(&mut self.timeout_ms, higher.timeout_ms);
        replace(&mut self.max_workers, higher.max_workers);
    }
}

impl TaskConfig {
    fn resolve_paths(&mut self, root: &Path) {
        match self {
            Self::Command(command) => {
                *self = Self::Detailed(TaskOptions {
                    command:      std::mem::take(command),
                    description:  None,
                    cwd:          Some(root.to_path_buf()),
                    dependencies: Vec::new(),
                    env:          BTreeMap::new(),
                });
            }
            Self::Detailed(options) => {
                if options.cwd.is_none() {
                    options.cwd = Some(root.to_path_buf());
                } else {
                    resolve_optional_path(root, &mut options.cwd);
                }
            }
        }
    }
}

impl WorkspaceConfig {
    fn resolve_paths(&mut self, root: &Path) {
        match self {
            Self::Members(members) => resolve_path_list(root, members),
            Self::Detailed(options) => {
                resolve_path_list(root, &mut options.members);
                resolve_path_list(root, &mut options.exclude);
            }
        }
    }
}

const fn jsonc_options() -> ParseOptions {
    ParseOptions {
        allow_comments:                    true,
        allow_trailing_commas:             true,
        allow_loose_object_property_names: false,
        allow_missing_commas:              false,
        allow_single_quoted_strings:       false,
        allow_hexadecimal_numbers:         false,
        allow_unary_plus_numbers:          false,
    }
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(ConfigError::CurrentDirectory)
    }
}

fn resolve_optional_path(root: &Path, path: &mut Option<PathBuf>) {
    if let Some(path) = path {
        resolve_path(root, path);
    }
}

fn resolve_import_targets(
    root: &Path, imports: &mut Option<BTreeMap<String, String>>,
) -> Result<()> {
    if let Some(imports) = imports {
        for target in imports.values_mut() {
            if target.starts_with("./") || target.starts_with("../") {
                let mut path = PathBuf::from(&*target);
                resolve_path(root, &mut path);
                *target = file_url(&path)?;
            }
        }
    }
    Ok(())
}

fn resolve_dependency_targets(
    root: &Path, dependencies: &mut Option<BTreeMap<String, String>>,
) -> Result<()> {
    if let Some(dependencies) = dependencies {
        for target in dependencies.values_mut() {
            for prefix in ["file:", "link:"] {
                if let Some(path) = target.strip_prefix(prefix)
                    && Path::new(path).is_relative()
                {
                    let mut path = PathBuf::from(path);
                    resolve_path(root, &mut path);
                    let url = file_url(&path)?;
                    *target = if prefix == "file:" {
                        url
                    } else {
                        format!("link:{url}")
                    };
                    break;
                }
            }
        }
    }
    Ok(())
}

fn file_url(path: &Path) -> Result<String> {
    Url::from_file_path(path).map(String::from).map_err(|()| {
        ConfigError::InvalidFileUrl {
            path: path.to_path_buf(),
        }
    })
}

fn resolve_paths(root: &Path, paths: &mut Option<Vec<PathBuf>>) {
    if let Some(paths) = paths {
        resolve_path_list(root, paths);
    }
}

fn resolve_access_paths(
    root: &Path, source_path: &Path, capability: &'static str, access: &mut Option<Access<PathBuf>>,
) -> Result<()> {
    if let Some(Access::List(paths)) = access {
        validate_and_resolve_permission_paths(root, source_path, capability, paths)?;
    } else if let Some(Access::Rules(rules)) = access {
        for value in [&mut rules.allow, &mut rules.deny] {
            if let Some(AccessValue::List(paths)) = value {
                validate_and_resolve_permission_paths(root, source_path, capability, paths)?;
            }
        }
    }
    Ok(())
}

fn validate_and_resolve_permission_paths(
    root: &Path, source_path: &Path, capability: &'static str, paths: &mut [PathBuf],
) -> Result<()> {
    if paths.iter().any(|path| path.as_os_str().is_empty()) {
        return Err(ConfigError::EmptyPermissionPath {
            path: source_path.to_path_buf(),
            capability,
        });
    }
    resolve_path_list(root, paths);
    Ok(())
}

fn resolve_path_list(root: &Path, paths: &mut [PathBuf]) {
    for path in paths {
        resolve_path(root, path);
    }
}

fn resolve_path(root: &Path, path: &mut PathBuf) {
    *path = lexical_normalize(if path.is_relative() {
        root.join(&*path)
    } else {
        std::mem::take(path)
    });
}

fn lexical_normalize(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn replace<T>(lower: &mut Option<T>, higher: Option<T>) {
    if higher.is_some() {
        *lower = higher;
    }
}

fn merge_map<K: Ord, V>(lower: &mut Option<BTreeMap<K, V>>, higher: Option<BTreeMap<K, V>>) {
    if let Some(higher) = higher {
        lower.get_or_insert_default().extend(higher);
    }
}

fn merge_nested<T>(lower: &mut Option<T>, higher: Option<T>, merge: impl FnOnce(&mut T, T)) {
    if let Some(higher) = higher {
        if let Some(lower) = lower {
            merge(lower, higher);
        } else {
            *lower = Some(higher);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::Path};

    use den_capabilities::{Decision, Request};
    use tempfile::tempdir;

    use super::{
        Access, Config, ConfigError, ParseOptions, RawConfig, TaskConfig, WorkspaceConfig, file_url,
    };

    fn write(path: &Path, text: &str) { fs::write(path, text).expect("write test configuration"); }

    #[test]
    fn loads_jsonc_and_resolves_every_path_from_the_config_root() {
        let temp = tempdir().expect("create temp directory");
        let path = temp.path().join("den.jsonc");
        write(
            &path,
            r#"{
                // JSONC comments and trailing commas are intentional.
                "importMap": "config/import-map.json",
                "packageStore": ".den/packages.db",
                "envFiles": [".env"],
                "preloads": ["boot.ts"],
                "permissions": {
                    "read": ["data"],
                    "ffi": ["native/plugin.so"],
                },
                "tasks": {
                    "dev": { "command": "den run main.ts", "cwd": "app" },
                },
                "workspace": { "members": ["a"], "exclude": ["a/tmp"] },
            }"#,
        );

        let config = Config::load(&path).expect("load JSONC");
        assert_eq!(
            config.import_map,
            Some(temp.path().join("config/import-map.json"))
        );
        assert_eq!(
            config.package_store,
            Some(temp.path().join(".den/packages.db"))
        );
        assert_eq!(config.env_files, Some(vec![temp.path().join(".env")]));
        assert_eq!(config.preloads, Some(vec![temp.path().join("boot.ts")]));
        assert_eq!(
            config
                .permissions
                .as_ref()
                .and_then(|value| value.read.as_ref()),
            Some(&Access::List(vec![temp.path().join("data")]))
        );
        let tasks = config.tasks.as_ref().expect("tasks configured");
        let TaskConfig::Detailed(task) = tasks.get("dev").expect("dev task") else {
            panic!("dev task should be detailed")
        };
        assert_eq!(task.cwd, Some(temp.path().join("app")));
        let WorkspaceConfig::Detailed(workspace) = config.workspace.expect("workspace") else {
            panic!("workspace should be detailed")
        };
        assert_eq!(workspace.members, vec![temp.path().join("a")]);
        assert_eq!(workspace.exclude, vec![temp.path().join("a/tmp")]);
    }

    #[test]
    fn discovers_upward_and_explicit_paths_take_precedence() {
        let temp = tempdir().expect("create temp directory");
        let nested = temp.path().join("a/b");
        fs::create_dir_all(&nested).expect("create nested directory");
        let discovered_path = temp.path().join("den.jsonc");
        write(&discovered_path, r#"{ "offline": true }"#);
        let explicit_path = nested.join("explicit.json");
        write(&explicit_path, r#"{ "offline": false }"#);

        let discovered = Config::discover(&nested, None)
            .expect("discover configuration")
            .expect("configuration exists");
        assert_eq!(
            discovered.source().map(|source| source.path.as_path()),
            Some(discovered_path.as_path())
        );
        assert_eq!(discovered.offline, Some(true));

        let explicit = Config::discover(&nested, Some(&explicit_path))
            .expect("load explicit configuration")
            .expect("configuration exists");
        assert_eq!(explicit.offline, Some(false));
    }

    #[test]
    fn malformed_and_unknown_fields_report_the_file() {
        let temp = tempdir().expect("create temp directory");
        let malformed = temp.path().join("den.json");
        write(&malformed, r#"{ "offline": "yes" }"#);

        let error = Config::load(&malformed).expect_err("wrong field type should fail");
        assert!(matches!(error, ConfigError::Parse { .. }));
        let message = error.to_string();
        assert!(message.contains(malformed.to_string_lossy().as_ref()));
        assert!(message.contains("boolean"));

        write(&malformed, r#"{ "offine": true }"#);
        let error = Config::load(&malformed).expect_err("unknown field should fail");
        assert!(error.to_string().contains("unknown field `offine`"));
    }

    #[test]
    fn empty_permission_paths_fail_closed_before_resolution() {
        let temp = tempdir().expect("create temp directory");
        let path = temp.path().join("den.json");
        write(
            &path,
            r#"{ "permissions": { "read": { "allow": true, "deny": [""] } } }"#,
        );

        let error = Config::load(&path).expect_err("empty permission path must fail");
        assert!(matches!(error, ConfigError::EmptyPermissionPath {
            capability: "read",
            ..
        }));
    }

    #[test]
    fn higher_priority_layers_merge_maps_and_replace_explicit_values() {
        let lower: RawConfig = jsonc_parser::parse_to_serde_value(
            r#"{
                "imports": { "a": "./a.ts", "same": "./old.ts" },
                "permissions": { "read": true, "netConnect": ["example.com"] },
                "budgets": { "heapBytes": 10, "timeoutMs": 20 },
                "envFiles": ["base.env"],
                "offline": true
            }"#,
            &ParseOptions::default(),
        )
        .expect("parse lower layer");
        let higher: RawConfig = jsonc_parser::parse_to_serde_value(
            r#"{
                "imports": { "b": "./b.ts", "same": "./new.ts" },
                "permissions": { "read": false },
                "budgets": { "timeoutMs": 5 },
                "envFiles": [],
                "offline": false
            }"#,
            &ParseOptions::default(),
        )
        .expect("parse higher layer");

        let merged = Config::from(lower).merged(Config::from(higher));
        assert_eq!(
            merged.imports,
            Some(BTreeMap::from([
                ("a".into(), "./a.ts".into()),
                ("b".into(), "./b.ts".into()),
                ("same".into(), "./new.ts".into()),
            ]))
        );
        let permissions = merged.permissions.expect("permissions remain configured");
        assert_eq!(permissions.read, Some(Access::All(false)));
        assert_eq!(
            permissions.net_connect,
            Some(Access::List(vec!["example.com".into()]))
        );
        let budgets = merged.budgets.expect("budgets remain configured");
        assert_eq!(budgets.heap_bytes, Some(10));
        assert_eq!(budgets.timeout_ms, Some(5));
        assert_eq!(merged.env_files, Some(Vec::new()));
        assert_eq!(merged.offline, Some(false));
    }

    #[test]
    fn merged_loaded_configs_keep_each_entrys_declaring_root() {
        let temp = tempdir().expect("create temp directory");
        let lower_root = temp.path().join("lower");
        let higher_root = temp.path().join("higher");
        fs::create_dir_all(&lower_root).expect("create lower root");
        fs::create_dir_all(&higher_root).expect("create higher root");
        let lower_path = lower_root.join("den.json");
        let higher_path = higher_root.join("den.json");
        write(
            &lower_path,
            r#"{
                "imports": { "lower": "./lower.ts" },
                "dependencies": { "local": "file:./pkg" },
                "tasks": { "lower": "den run lower.ts" }
            }"#,
        );
        write(
            &higher_path,
            r#"{
                "imports": { "higher": "./higher.ts" },
                "tasks": { "higher": "den run higher.ts" }
            }"#,
        );

        let merged = Config::load(lower_path)
            .expect("load lower")
            .merged(Config::load(higher_path).expect("load higher"));
        let imports = merged.imports.expect("imports");
        assert_eq!(
            imports.get("lower"),
            Some(&file_url(&lower_root.join("lower.ts")).expect("lower file URL"))
        );
        assert_eq!(
            imports.get("higher"),
            Some(&file_url(&higher_root.join("higher.ts")).expect("higher file URL"))
        );
        assert_eq!(
            merged
                .dependencies
                .and_then(|values| values.get("local").cloned()),
            Some(file_url(&lower_root.join("pkg")).expect("package file URL"))
        );
        for (name, root) in [("lower", lower_root), ("higher", higher_root)] {
            let TaskConfig::Detailed(task) = merged
                .tasks
                .as_ref()
                .and_then(|tasks| tasks.get(name))
                .expect("task")
            else {
                panic!("loaded task should carry its root")
            };
            assert_eq!(task.cwd.as_deref(), Some(root.as_path()));
        }
    }

    #[test]
    fn permissions_build_a_scoped_deny_by_default_policy() {
        let temp = tempdir().expect("create temp directory");
        let path = temp.path().join("den.json");
        write(
            &path,
            r#"{
                "permissions": {
                    "read": { "allow": ["data"], "deny": ["data/private"] },
                    "env": ["PUBLIC_TOKEN"],
                    "netConnect": ["example.com:443", "10.0.0.0/8"]
                }
            }"#,
        );
        let policy = Config::load(path)
            .expect("load configuration")
            .policy()
            .expect("build policy");

        let allowed = Request::read(temp.path().join("data/file.txt")).expect("read request");
        let denied = Request::read(temp.path().join("data/private/key")).expect("read request");
        let outside = Request::read(temp.path().join("other.txt")).expect("read request");
        assert_eq!(policy.query(&allowed).decision(), Decision::Allowed);
        assert_eq!(policy.query(&denied).decision(), Decision::Denied);
        assert_eq!(policy.query(&outside).decision(), Decision::Denied);
        assert!(
            policy
                .check(&Request::env("PUBLIC_TOKEN").expect("env request"))
                .is_ok()
        );
        assert!(
            policy
                .check(&Request::env("SECRET").expect("env request"))
                .is_err()
        );
        assert!(
            policy
                .check(&Request::net_connect("example.com", 443).expect("network request"))
                .is_ok()
        );
        assert!(
            policy
                .check(&Request::net_connect("example.com", 80).expect("network request"))
                .is_err()
        );
        assert!(
            policy
                .check(&Request::net_connect("10.2.3.4", 80).expect("network request"))
                .is_ok()
        );
    }
}
