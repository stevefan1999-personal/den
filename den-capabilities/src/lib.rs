//! Den's deny-by-default capability policy kernel.
//!
//! A [`Policy`] is one layer of allow and deny rules. [`Policy::attenuate`]
//! keeps parent and child policies as conjunctive layers: every layer must
//! allow a request, while a deny in any layer wins. Keeping the intersection
//! logical avoids lossy scope rewriting and makes it impossible for a child
//! to broaden its parent.
//!
//! Path matching here is deliberately lexical. [`NormalizedPath`] removes
//! `.` and `..`, requires an absolute path, and uses path-component boundaries,
//! but it cannot stop symlink swaps or other TOCTOU attacks. Filesystem and FFI
//! enforcement must later resolve and operate on descriptor-relative paths
//! (for example, `openat2`/`cap-std` style), not authorize a string and then
//! reopen that string by name.
//! Case aliases on case-insensitive filesystems likewise require authorization
//! against the canonical opened handle; lexical policy values are deliberately
//! not case-folded because individual directories may be case-sensitive.

#![deny(unsafe_code)]

use std::{
    fmt,
    net::IpAddr,
    path::{Component, Path, PathBuf},
    str::FromStr as _,
};

use ipnet::IpNet;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use thiserror::Error;
use url::{Host, Origin, Url};

/// A capability controlled by a policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Read,
    Write,
    NetConnect,
    NetListen,
    Env,
    Run,
    Sys,
    Ffi,
    Import,
    Secrets,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::NetConnect => "net-connect",
            Self::NetListen => "net-listen",
            Self::Env => "env",
            Self::Run => "run",
            Self::Sys => "sys",
            Self::Ffi => "ffi",
            Self::Import => "import",
            Self::Secrets => "secrets",
        })
    }
}

/// Whether a matching rule grants or rejects access.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    Allow,
    Deny,
}

/// The result of a policy query.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    Allowed,
    Denied,
}

impl Decision {
    #[must_use]
    pub const fn is_allowed(self) -> bool { matches!(self, Self::Allowed) }
}

/// Invalid policy or request input. Invalid input is never widened into an
/// all-access scope.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ScopeError {
    #[error("scope cannot be empty")]
    Empty,
    #[error("path scope must be absolute and cannot escape its root")]
    InvalidPath,
    #[error("name scope contains an invalid character")]
    InvalidName,
    #[error("host scope is malformed")]
    InvalidHost,
    #[error("network port range is reversed")]
    InvalidPortRange,
    #[error("CIDR scope is malformed")]
    InvalidCidr,
    #[error("wildcard addresses and port zero require bind-and-recheck enforcement")]
    DynamicListen,
    #[error("URL scope is malformed or would broaden unexpectedly")]
    InvalidUrl,
}

/// A normalized absolute path. Scope matching uses component boundaries, so
/// `/srv/app` does not match `/srv/application`.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NormalizedPath(PathBuf);

impl NormalizedPath {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, ScopeError> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Err(ScopeError::Empty);
        }
        if !path.is_absolute() {
            return Err(ScopeError::InvalidPath);
        }

        let mut normalized = PathBuf::new();
        let mut normal_components = 0_usize;
        for component in path.components() {
            match component {
                Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
                Component::RootDir => normalized.push(component.as_os_str()),
                Component::CurDir => {}
                Component::ParentDir => {
                    if normal_components == 0 {
                        return Err(ScopeError::InvalidPath);
                    }
                    normalized.pop();
                    normal_components -= 1;
                }
                Component::Normal(value) => {
                    normalized.push(value);
                    normal_components += 1;
                }
            }
        }

        if normalized.is_absolute() {
            Ok(Self(normalized))
        } else {
            Err(ScopeError::InvalidPath)
        }
    }

    #[must_use]
    pub fn as_path(&self) -> &Path { &self.0 }

    #[must_use]
    pub fn contains(&self, requested: &Self) -> bool { requested.0.starts_with(&self.0) }
}

impl<'de> Deserialize<'de> for NormalizedPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(PathBuf::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl fmt::Display for NormalizedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.display().fmt(f) }
}

/// A non-empty environment, system-information, or secret name.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ResourceName(String);

impl ResourceName {
    pub fn new<S: Into<String>>(value: S) -> Result<Self, ScopeError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ScopeError::Empty);
        }
        if value.contains('\0') || value.contains('=') {
            return Err(ScopeError::InvalidName);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}

impl<'de> Deserialize<'de> for ResourceName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl fmt::Display for ResourceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

/// Exact or prefix matching for named resources.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "match", content = "value", rename_all = "kebab-case")]
pub enum NameScope {
    Exact(ResourceName),
    Prefix(ResourceName),
}

impl NameScope {
    pub fn exact<S: Into<String>>(value: S) -> Result<Self, ScopeError> {
        ResourceName::new(value).map(Self::Exact)
    }

    pub fn prefix<S: Into<String>>(value: S) -> Result<Self, ScopeError> {
        ResourceName::new(value).map(Self::Prefix)
    }

    #[must_use]
    pub fn matches(&self, requested: &ResourceName) -> bool {
        match self {
            Self::Exact(value) => value == requested,
            Self::Prefix(value) => requested.0.starts_with(&value.0),
        }
    }

    #[must_use]
    pub fn matches_env(&self, requested: &ResourceName) -> bool {
        if cfg!(windows) {
            match self {
                Self::Exact(value) => value.0.eq_ignore_ascii_case(&requested.0),
                Self::Prefix(value) => {
                    requested
                        .0
                        .get(..value.0.len())
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&value.0))
                }
            }
        } else {
            self.matches(requested)
        }
    }
}

/// A normalized DNS name or IP address.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NormalizedHost(String);

impl NormalizedHost {
    pub fn new<S: AsRef<str>>(value: S) -> Result<Self, ScopeError> {
        let mut value = value.as_ref();
        if value.is_empty() {
            return Err(ScopeError::Empty);
        }
        if value.ends_with('.') {
            value = value.trim_end_matches('.');
            if value.is_empty() {
                return Err(ScopeError::InvalidHost);
            }
        }
        let normalized = value.parse::<IpAddr>().map_or_else(
            |_error| {
                Host::parse(value)
                    .map(|host| {
                        match host {
                            Host::Domain(domain) => domain,
                            Host::Ipv4(address) => address.to_string(),
                            Host::Ipv6(address) => normalize_ip(IpAddr::V6(address)).to_string(),
                        }
                    })
                    .map_err(|_error| ScopeError::InvalidHost)
            },
            |address| Ok(normalize_ip(address).to_string()),
        )?;
        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_ip_addr(&self) -> Option<IpAddr> { self.0.parse().ok() }

    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}

impl<'de> Deserialize<'de> for NormalizedHost {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl fmt::Display for NormalizedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

/// A canonical CIDR network.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Cidr(IpNet);

impl Cidr {
    pub fn new(network: IpNet) -> Result<Self, ScopeError> {
        if matches!(network, IpNet::V6(network) if network.addr().to_ipv4_mapped().is_some()) {
            return Err(ScopeError::InvalidCidr);
        }
        Ok(Self(network.trunc()))
    }

    pub fn parse(value: &str) -> Result<Self, ScopeError> {
        if value.is_empty() {
            return Err(ScopeError::Empty);
        }
        IpNet::from_str(value)
            .map_err(|_error| ScopeError::InvalidCidr)
            .and_then(Self::new)
    }

    #[must_use]
    pub const fn network(self) -> IpNet { self.0 }

    #[must_use]
    pub fn contains(self, address: &IpAddr) -> bool {
        let normalized = match address {
            IpAddr::V6(address) => address.to_ipv4_mapped().map(IpAddr::V4),
            IpAddr::V4(_) => None,
        };
        self.0.contains(normalized.as_ref().unwrap_or(address))
    }
}

impl Serialize for Cidr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Cidr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl fmt::Display for Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) }
}

/// An inclusive network port range.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PortRangeWire")]
pub struct PortRange {
    start: u16,
    end:   u16,
}

#[derive(Deserialize)]
struct PortRangeWire {
    start: u16,
    end:   u16,
}

impl PortRange {
    pub const fn new(start: u16, end: u16) -> Result<Self, ScopeError> {
        if start > end {
            Err(ScopeError::InvalidPortRange)
        } else {
            Ok(Self { start, end })
        }
    }

    #[must_use]
    pub const fn exact(port: u16) -> Self {
        Self {
            start: port,
            end:   port,
        }
    }

    #[must_use]
    pub const fn start(self) -> u16 { self.start }

    #[must_use]
    pub const fn end(self) -> u16 { self.end }

    #[must_use]
    pub fn contains(self, port: u16) -> bool { (self.start..=self.end).contains(&port) }
}

impl TryFrom<PortRangeWire> for PortRange {
    type Error = ScopeError;

    fn try_from(value: PortRangeWire) -> Result<Self, Self::Error> {
        Self::new(value.start, value.end)
    }
}

/// A host or network combined with an inclusive port range.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct NetworkScope {
    host:  HostScope,
    ports: PortRange,
}

impl NetworkScope {
    pub fn host<S: AsRef<str>>(host: S, ports: PortRange) -> Result<Self, ScopeError> {
        Ok(Self {
            host: HostScope::Exact(NormalizedHost::new(host)?),
            ports,
        })
    }

    pub fn cidr<S: AsRef<str>>(cidr: S, ports: PortRange) -> Result<Self, ScopeError> {
        Ok(Self {
            host: HostScope::Cidr(Cidr::parse(cidr.as_ref())?),
            ports,
        })
    }

    #[must_use]
    pub const fn host_scope(&self) -> &HostScope { &self.host }

    #[must_use]
    pub const fn ports(&self) -> PortRange { self.ports }

    #[must_use]
    pub fn matches(&self, requested: &NetworkTarget, effect: Effect) -> bool {
        self.ports.contains(requested.port) && self.host.matches(requested, effect)
    }
}

/// Host matching for a network rule. CIDR rules match concrete IP targets;
/// an enforcer resolving DNS must check every resolved address before use.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "match", content = "value", rename_all = "kebab-case")]
pub enum HostScope {
    Exact(NormalizedHost),
    Cidr(Cidr),
}

impl HostScope {
    fn matches(&self, requested: &NetworkTarget, effect: Effect) -> bool {
        match self {
            Self::Exact(host) if host.as_ip_addr().is_none() => host == &requested.host,
            Self::Exact(host) => {
                host.as_ip_addr().is_some_and(|scope| {
                    let scope = normalize_ip(scope);
                    requested.matches_resolved(effect, |address| normalize_ip(address) == scope)
                })
            }
            Self::Cidr(network) => {
                requested.matches_resolved(effect, |address| network.contains(&address))
            }
        }
    }
}

/// A concrete host and port requested by a network operation.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
pub struct NetworkTarget {
    host:     NormalizedHost,
    port:     u16,
    #[serde(default)]
    resolved: Vec<IpAddr>,
}

impl NetworkTarget {
    /// Construct a target before DNS resolution. IP/CIDR deny rules fail
    /// closed for domain targets until [`Self::with_resolved`] supplies the
    /// concrete addresses.
    fn new<S: AsRef<str>>(host: S, port: u16) -> Result<Self, ScopeError> {
        Ok(Self {
            host: NormalizedHost::new(host)?,
            port,
            resolved: Vec::new(),
        })
    }

    /// Construct the target used for the final connect authorization check.
    /// Every address returned by DNS must be included.
    pub fn with_resolved<S: AsRef<str>, I: IntoIterator<Item = IpAddr>>(
        host: S, port: u16, resolved: I,
    ) -> Result<Self, ScopeError> {
        let mut target = Self::new(host, port)?;
        target.resolved = resolved.into_iter().map(normalize_ip).collect();
        target.resolved.sort_unstable();
        target.resolved.dedup();
        if target
            .host
            .as_ip_addr()
            .is_some_and(|address| address.is_unspecified())
            || target.resolved.iter().any(IpAddr::is_unspecified)
        {
            return Err(ScopeError::InvalidHost);
        }
        Ok(target)
    }

    #[must_use]
    pub const fn host(&self) -> &NormalizedHost { &self.host }

    #[must_use]
    pub const fn port(&self) -> u16 { self.port }

    #[must_use]
    pub fn resolved(&self) -> &[IpAddr] { &self.resolved }

    fn matches_resolved(&self, effect: Effect, predicate: impl Fn(IpAddr) -> bool) -> bool {
        if let Some(address) = self.host.as_ip_addr().map(normalize_ip) {
            return predicate(address);
        }
        if self.resolved.is_empty() {
            // An unresolved hostname cannot prove it avoids an IP/CIDR deny.
            // Callers may do an initial hostname-only check, but the final
            // connect check must provide every resolved address.
            return matches!(effect, Effect::Deny);
        }
        match effect {
            Effect::Allow => self.resolved.iter().copied().all(predicate),
            Effect::Deny => self.resolved.iter().copied().any(predicate),
        }
    }
}

fn normalize_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => {
            address
                .to_ipv4_mapped()
                .map_or(IpAddr::V6(address), IpAddr::V4)
        }
        IpAddr::V4(_) => address,
    }
}

impl fmt::Display for NetworkTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if matches!(self.host.as_ip_addr(), Some(IpAddr::V6(_))) {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// A canonical, non-opaque URL origin.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UrlOrigin(String);

impl UrlOrigin {
    pub fn new<S: AsRef<str>>(value: S) -> Result<Self, ScopeError> {
        let target = ImportTarget::new(value)?;
        let url = target.as_url();
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(ScopeError::InvalidUrl);
        }
        match url.origin() {
            Origin::Tuple(_, _, _) => Ok(Self(url.origin().ascii_serialization())),
            Origin::Opaque(_) => Err(ScopeError::InvalidUrl),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }

    #[must_use]
    pub fn matches(&self, target: &ImportTarget) -> bool {
        self.0
            .split_once(':')
            .is_some_and(|(scheme, _)| scheme == target.0.scheme())
            && target.0.origin().ascii_serialization() == self.0
    }
}

impl<'de> Deserialize<'de> for UrlOrigin {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl fmt::Display for UrlOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

/// A canonical hierarchical URL prefix with path-component matching.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UrlPrefix(Url);

impl UrlPrefix {
    pub fn new<S: AsRef<str>>(value: S) -> Result<Self, ScopeError> {
        let target = ImportTarget::new(value)?;
        if target.0.cannot_be_a_base()
            || target.0.query().is_some()
            || target.0.fragment().is_some()
        {
            return Err(ScopeError::InvalidUrl);
        }
        Ok(Self(target.0))
    }

    #[must_use]
    pub const fn as_url(&self) -> &Url { &self.0 }

    #[must_use]
    pub fn matches(&self, target: &ImportTarget) -> bool {
        if !same_url_authority(&self.0, &target.0) {
            return false;
        }
        let scope = self.0.path();
        let requested = target.0.path();
        requested == scope
            || (scope.ends_with('/') && requested.starts_with(scope))
            || requested
                .strip_prefix(scope)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }
}

impl<'de> Deserialize<'de> for UrlPrefix {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl fmt::Display for UrlPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.0.as_str()) }
}

/// Origin-wide or hierarchical-prefix matching for module imports.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "match", content = "value", rename_all = "kebab-case")]
pub enum ImportScope {
    Exact(ImportTarget),
    Origin(UrlOrigin),
    Prefix(UrlPrefix),
}

impl ImportScope {
    pub fn exact<S: AsRef<str>>(value: S) -> Result<Self, ScopeError> {
        ImportTarget::new(value).map(Self::Exact)
    }

    pub fn origin<S: AsRef<str>>(value: S) -> Result<Self, ScopeError> {
        UrlOrigin::new(value).map(Self::Origin)
    }

    pub fn prefix<S: AsRef<str>>(value: S) -> Result<Self, ScopeError> {
        UrlPrefix::new(value).map(Self::Prefix)
    }

    #[must_use]
    pub fn matches(&self, requested: &ImportTarget) -> bool {
        match self {
            Self::Exact(scope) => scope == requested,
            Self::Origin(scope) => scope.matches(requested),
            Self::Prefix(scope) => scope.matches(requested),
        }
    }
}

/// A canonical URL requested by module loading.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ImportTarget(Url);

impl ImportTarget {
    pub fn new<S: AsRef<str>>(value: S) -> Result<Self, ScopeError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(ScopeError::Empty);
        }
        let mut url = Url::parse(value).map_err(|_error| ScopeError::InvalidUrl)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ScopeError::InvalidUrl);
        }
        url.set_fragment(None);
        if url.scheme() == "file" {
            let encoded = url.path().to_ascii_lowercase();
            if ["%2f", "%5c", "%2e"]
                .iter()
                .any(|needle| encoded.contains(needle))
            {
                return Err(ScopeError::InvalidUrl);
            }
            let path = url.to_file_path().map_err(|()| ScopeError::InvalidUrl)?;
            let path = NormalizedPath::new(path)?;
            url = Url::from_file_path(path.as_path()).map_err(|()| ScopeError::InvalidUrl)?;
        }
        Ok(Self(url))
    }

    #[must_use]
    pub const fn as_url(&self) -> &Url { &self.0 }
}

impl<'de> Deserialize<'de> for ImportTarget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl fmt::Display for ImportTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.0.as_str()) }
}

fn same_url_authority(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host() == right.host()
        && left.port_or_known_default() == right.port_or_known_default()
}

/// The normalized resource scope of one rule.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "capability", content = "scope", rename_all = "kebab-case")]
pub enum Scope {
    All(Capability),
    Read(NormalizedPath),
    Write(NormalizedPath),
    NetConnect(NetworkScope),
    NetListen(NetworkScope),
    Env(NameScope),
    Run(NormalizedPath),
    Sys(NameScope),
    Ffi(NormalizedPath),
    Import(ImportScope),
    Secrets(NameScope),
}

impl Scope {
    #[must_use]
    pub const fn capability(&self) -> Capability {
        match self {
            Self::All(capability) => *capability,
            Self::Read(_) => Capability::Read,
            Self::Write(_) => Capability::Write,
            Self::NetConnect(_) => Capability::NetConnect,
            Self::NetListen(_) => Capability::NetListen,
            Self::Env(_) => Capability::Env,
            Self::Run(_) => Capability::Run,
            Self::Sys(_) => Capability::Sys,
            Self::Ffi(_) => Capability::Ffi,
            Self::Import(_) => Capability::Import,
            Self::Secrets(_) => Capability::Secrets,
        }
    }

    #[must_use]
    fn matches(&self, effect: Effect, requested: &Request) -> bool {
        match (self, requested) {
            (Self::All(scope), request) => *scope == request.capability(),
            (Self::Read(scope), Request::Read(target))
            | (Self::Write(scope), Request::Write(target))
            | (Self::Run(scope), Request::Run(target))
            | (Self::Ffi(scope), Request::Ffi(target)) => scope.contains(target),
            (Self::NetConnect(scope), Request::NetConnect(target))
            | (Self::NetListen(scope), Request::NetListen(target)) => scope.matches(target, effect),
            (Self::Env(scope), Request::Env(target)) => scope.matches_env(target),
            (Self::Sys(scope), Request::Sys(target))
            | (Self::Secrets(scope), Request::Secrets(target)) => scope.matches(target),
            (Self::Import(scope), Request::Import(target)) => scope.matches(target),
            _ => false,
        }
    }

    #[must_use]
    fn is_all_for(&self, capability: Capability) -> bool {
        matches!(self, Self::All(scope) if *scope == capability)
    }
}

/// A concrete capability request.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "capability", content = "resource", rename_all = "kebab-case")]
pub enum Request {
    Read(NormalizedPath),
    Write(NormalizedPath),
    NetConnect(NetworkTarget),
    NetListen(NetworkTarget),
    Env(ResourceName),
    Run(NormalizedPath),
    Sys(ResourceName),
    Ffi(NormalizedPath),
    Import(ImportTarget),
    Secrets(ResourceName),
}

impl Request {
    pub fn read<P: AsRef<Path>>(path: P) -> Result<Self, ScopeError> {
        NormalizedPath::new(path).map(Self::Read)
    }

    pub fn write<P: AsRef<Path>>(path: P) -> Result<Self, ScopeError> {
        NormalizedPath::new(path).map(Self::Write)
    }

    pub fn net_connect<S: AsRef<str>>(host: S, port: u16) -> Result<Self, ScopeError> {
        let target = NetworkTarget::new(host, port)?;
        if target
            .host
            .as_ip_addr()
            .is_some_and(|address| address.is_unspecified())
        {
            return Err(ScopeError::InvalidHost);
        }
        Ok(Self::NetConnect(target))
    }

    pub fn net_listen<S: AsRef<str>>(host: S, port: u16) -> Result<Self, ScopeError> {
        let target = NetworkTarget::new(host, port)?;
        if port == 0
            || target
                .host
                .as_ip_addr()
                .is_some_and(|address| address.is_unspecified())
        {
            return Err(ScopeError::DynamicListen);
        }
        Ok(Self::NetListen(target))
    }

    pub fn env<S: Into<String>>(name: S) -> Result<Self, ScopeError> {
        ResourceName::new(name).map(Self::Env)
    }

    pub fn run_path<P: AsRef<Path>>(path: P) -> Result<Self, ScopeError> {
        NormalizedPath::new(path).map(Self::Run)
    }

    pub fn sys<S: Into<String>>(name: S) -> Result<Self, ScopeError> {
        ResourceName::new(name).map(Self::Sys)
    }

    pub fn ffi<P: AsRef<Path>>(path: P) -> Result<Self, ScopeError> {
        NormalizedPath::new(path).map(Self::Ffi)
    }

    pub fn import<S: AsRef<str>>(url: S) -> Result<Self, ScopeError> {
        ImportTarget::new(url).map(Self::Import)
    }

    pub fn secret<S: Into<String>>(name: S) -> Result<Self, ScopeError> {
        ResourceName::new(name).map(Self::Secrets)
    }

    #[must_use]
    pub const fn capability(&self) -> Capability {
        match self {
            Self::Read(_) => Capability::Read,
            Self::Write(_) => Capability::Write,
            Self::NetConnect(_) => Capability::NetConnect,
            Self::NetListen(_) => Capability::NetListen,
            Self::Env(_) => Capability::Env,
            Self::Run(_) => Capability::Run,
            Self::Sys(_) => Capability::Sys,
            Self::Ffi(_) => Capability::Ffi,
            Self::Import(_) => Capability::Import,
            Self::Secrets(_) => Capability::Secrets,
        }
    }
}

impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(value) | Self::Write(value) | Self::Run(value) | Self::Ffi(value) => {
                value.fmt(f)
            }
            Self::NetConnect(value) | Self::NetListen(value) => value.fmt(f),
            Self::Env(value) | Self::Sys(value) | Self::Secrets(value) => value.fmt(f),
            Self::Import(value) => value.fmt(f),
        }
    }
}

/// A stable, typed denial returned by [`Policy::check`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PermissionDenied {
    pub capability: Capability,
    pub requested:  Request,
}

impl PermissionDenied {
    /// A machine-readable identifier that is stable across message changes.
    pub const CODE: &'static str = "DEN_CAPABILITY_DENIED";
}

impl fmt::Display for PermissionDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} capability denied for {}",
            self.capability, self.requested
        )
    }
}

impl std::error::Error for PermissionDenied {}

/// One explicit allow or deny rule.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    effect: Effect,
    scope:  Scope,
}

impl Rule {
    #[must_use]
    pub const fn allow(scope: Scope) -> Self {
        Self {
            effect: Effect::Allow,
            scope,
        }
    }

    #[must_use]
    pub const fn deny(scope: Scope) -> Self {
        Self {
            effect: Effect::Deny,
            scope,
        }
    }

    #[must_use]
    pub const fn effect(&self) -> Effect { self.effect }

    #[must_use]
    pub const fn scope(&self) -> &Scope { &self.scope }

    fn matches(&self, requested: &Request) -> bool { self.scope.matches(self.effect, requested) }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Layer {
    rules: Vec<Rule>,
}

/// A serializable policy. Empty policies deny every request.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    layers: Vec<Layer>,
}

#[cfg(feature = "rquickjs")]
#[expect(
    unsafe_code,
    reason = "Policy owns only Rust values and borrows no JavaScript lifetime"
)]
unsafe impl rquickjs::JsLifetime<'_> for Policy {
    type Changed<'to> = Policy;
}

impl Policy {
    #[must_use]
    pub fn new<I: IntoIterator<Item = Rule>>(rules: I) -> Self {
        Self {
            layers: vec![Layer {
                rules: rules.into_iter().collect(),
            }],
        }
    }

    #[must_use]
    pub fn allow_all<I: IntoIterator<Item = Capability>>(capabilities: I) -> Self {
        Self::new(
            capabilities
                .into_iter()
                .map(|capability| Rule::allow(Scope::All(capability))),
        )
    }

    /// Intersect this parent with a child policy. Neither an empty policy nor
    /// a child allow can broaden the parent.
    #[must_use]
    pub fn attenuate(&self, child: &Self) -> Self {
        let mut layers = self.effective_layers();
        layers.extend(child.effective_layers());
        Self { layers }
    }

    pub fn check(&self, requested: &Request) -> Result<(), PermissionDenied> {
        if self.query(requested).decision().is_allowed() {
            Ok(())
        } else {
            Err(PermissionDenied {
                capability: requested.capability(),
                requested:  requested.clone(),
            })
        }
    }

    /// Query one concrete resource and retain access to all and matching
    /// rules without allocating a diagnostic copy.
    #[must_use]
    pub fn query<'policy, 'request>(
        &'policy self, requested: &'request Request,
    ) -> QueryResult<'policy, 'request> {
        QueryResult {
            policy: self,
            requested,
            decision: self.decision(requested),
        }
    }

    /// Whether every possible resource of `capability` is allowed. A scoped
    /// allow cannot prove this, and any scoped or global deny makes it false.
    #[must_use]
    pub fn query_all(&self, capability: Capability) -> Decision {
        if self.layers.is_empty() {
            return Decision::Denied;
        }
        for layer in &self.layers {
            let mut allowed = false;
            for rule in &layer.rules {
                if rule.scope.capability() != capability {
                    continue;
                }
                match rule.effect {
                    Effect::Deny => return Decision::Denied,
                    Effect::Allow if rule.scope.is_all_for(capability) => allowed = true,
                    Effect::Allow => {}
                }
            }
            if !allowed {
                return Decision::Denied;
            }
        }
        Decision::Allowed
    }

    /// Every configured rule, including rules that do not match a particular
    /// request.
    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.layers.iter().flat_map(|layer| &layer.rules)
    }

    fn decision(&self, requested: &Request) -> Decision {
        if self.layers.is_empty() {
            return Decision::Denied;
        }
        for layer in &self.layers {
            let mut allowed = false;
            for rule in &layer.rules {
                if !rule.matches(requested) {
                    continue;
                }
                match rule.effect {
                    Effect::Deny => return Decision::Denied,
                    Effect::Allow => allowed = true,
                }
            }
            if !allowed {
                return Decision::Denied;
            }
        }
        Decision::Allowed
    }

    fn effective_layers(&self) -> Vec<Layer> {
        if self.layers.is_empty() {
            vec![Layer { rules: Vec::new() }]
        } else {
            self.layers.clone()
        }
    }
}

/// A borrowed concrete-resource query result.
#[derive(Debug)]
pub struct QueryResult<'policy, 'request> {
    policy:    &'policy Policy,
    requested: &'request Request,
    decision:  Decision,
}

impl QueryResult<'_, '_> {
    #[must_use]
    pub const fn decision(&self) -> Decision { self.decision }

    #[must_use]
    pub const fn requested(&self) -> &Request { self.requested }

    /// All policy rules, useful for explaining why no rule matched.
    pub fn all_rules(&self) -> impl Iterator<Item = &Rule> { self.policy.rules() }

    /// Only rules whose capability and normalized scope match this request.
    pub fn matching_rules(&self) -> impl Iterator<Item = &Rule> {
        self.policy
            .rules()
            .filter(|rule| rule.matches(self.requested))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf { std::env::temp_dir().join("den-capabilities").join(name) }

    fn read_scope(path: &Path) -> Scope { Scope::Read(NormalizedPath::new(path).unwrap()) }

    #[test]
    fn default_is_denied_and_error_is_typed() {
        let request = Request::read(test_root("file.js")).unwrap();
        let error = Policy::default().check(&request).unwrap_err();

        assert_eq!(error.capability, Capability::Read);
        assert_eq!(error.requested, request);
        assert_eq!(PermissionDenied::CODE, "DEN_CAPABILITY_DENIED");
    }

    #[test]
    fn matching_deny_wins_over_allow() {
        let root = test_root("app");
        let private = root.join("private");
        let policy = Policy::new([
            Rule::allow(read_scope(&root)),
            Rule::deny(read_scope(&private)),
        ]);
        let public_request = Request::read(root.join("public.js")).unwrap();
        let private_request = Request::read(private.join("key.js")).unwrap();

        assert_eq!(policy.query(&public_request).decision(), Decision::Allowed);
        assert_eq!(policy.query(&private_request).decision(), Decision::Denied);
        assert_eq!(policy.query(&private_request).all_rules().count(), 2);
        assert_eq!(policy.query(&private_request).matching_rules().count(), 2);
    }

    #[test]
    fn path_and_url_prefixes_use_component_boundaries() {
        let root = test_root("pkg");
        let policy = Policy::new([
            Rule::allow(read_scope(&root)),
            Rule::allow(Scope::Import(
                ImportScope::prefix("https://example.test/pkg").unwrap(),
            )),
        ]);

        assert!(
            policy
                .check(&Request::read(root.join("mod.js")).unwrap())
                .is_ok()
        );
        assert!(
            policy
                .check(&Request::read(test_root("pkg-attack/mod.js")).unwrap())
                .is_err()
        );
        assert!(
            policy
                .check(&Request::import("https://example.test/pkg/mod.js").unwrap())
                .is_ok()
        );
        assert!(
            policy
                .check(&Request::import("https://example.test/pkg-attack/mod.js").unwrap())
                .is_err()
        );
    }

    #[test]
    fn cidr_and_ports_both_match() {
        let network = NetworkScope::cidr("10.0.0.99/8", PortRange::new(443, 444).unwrap()).unwrap();
        let policy = Policy::new([Rule::allow(Scope::NetConnect(network))]);

        assert!(
            policy
                .check(&Request::net_connect("10.2.3.4", 443).unwrap())
                .is_ok()
        );
        assert!(
            policy
                .check(&Request::net_connect("10.2.3.4", 80).unwrap())
                .is_err()
        );
        assert!(
            policy
                .check(&Request::net_connect("11.2.3.4", 443).unwrap())
                .is_err()
        );
        assert!(
            policy
                .check(&Request::net_connect("example.test", 443).unwrap())
                .is_err()
        );
    }

    #[test]
    fn resolved_dns_and_mapped_ipv6_cannot_bypass_cidr_denies() {
        let allowed_host = NetworkScope::host("example.test", PortRange::exact(443)).unwrap();
        let private = NetworkScope::cidr("10.0.0.0/8", PortRange::exact(443)).unwrap();
        let policy = Policy::new([
            Rule::allow(Scope::NetConnect(allowed_host)),
            Rule::deny(Scope::NetConnect(private.clone())),
        ]);
        let resolved =
            NetworkTarget::with_resolved("example.test", 443, [IpAddr::from([10, 2, 3, 4])])
                .unwrap();
        assert!(policy.check(&Request::NetConnect(resolved)).is_err());
        assert!(
            policy
                .check(&Request::net_connect("example.test", 443).unwrap())
                .is_err()
        );

        let mapped = Policy::new([
            Rule::allow(Scope::All(Capability::NetConnect)),
            Rule::deny(Scope::NetConnect(private)),
        ]);
        assert!(
            mapped
                .check(&Request::net_connect("::ffff:10.2.3.4", 443).unwrap())
                .is_err()
        );
    }

    #[test]
    fn origin_does_not_match_scheme_port_or_subdomain_changes() {
        let policy = Policy::new([Rule::allow(Scope::Import(
            ImportScope::origin("https://example.test").unwrap(),
        ))]);

        assert!(
            policy
                .check(&Request::import("https://example.test:443/a.ts").unwrap())
                .is_ok()
        );
        for url in [
            "http://example.test/a.ts",
            "https://sub.example.test/a.ts",
            "https://example.test:8443/a.ts",
        ] {
            assert!(policy.check(&Request::import(url).unwrap()).is_err());
        }
    }

    #[test]
    fn exact_import_scopes_support_opaque_package_urls() {
        let policy = Policy::new([Rule::allow(Scope::Import(
            ImportScope::exact("jsr:@std/path@1").unwrap(),
        ))]);

        assert!(
            policy
                .check(&Request::import("jsr:@std/path@1").unwrap())
                .is_ok()
        );
        assert!(
            policy
                .check(&Request::import("jsr:@std/path@2").unwrap())
                .is_err()
        );
    }

    #[test]
    fn import_identity_rejects_encoded_file_traversal_and_ignores_fragments() {
        assert!(matches!(
            Request::import("file:///safe/%2f..%2fsecret"),
            Err(ScopeError::InvalidUrl)
        ));
        let policy = Policy::new([
            Rule::allow(Scope::Import(
                ImportScope::origin("https://example.test").unwrap(),
            )),
            Rule::deny(Scope::Import(
                ImportScope::exact("https://example.test/admin.js").unwrap(),
            )),
        ]);
        assert!(
            policy
                .check(&Request::import("https://example.test/admin.js#ignored").unwrap())
                .is_err()
        );
        assert!(
            policy
                .check(&Request::import("blob:https://example.test/id").unwrap())
                .is_err()
        );

        let file_url = Url::from_file_path(test_root("admin.js"))
            .expect("absolute test path")
            .to_string();
        assert_eq!(
            ImportTarget::new(format!("{file_url}?ignored")).unwrap(),
            ImportTarget::new(file_url).unwrap()
        );
    }

    #[test]
    fn connect_rejects_unspecified_addresses_and_mapped_cidr_scopes() {
        assert_eq!(
            Request::net_connect("0.0.0.0", 80),
            Err(ScopeError::InvalidHost)
        );
        assert_eq!(Request::net_connect("::", 80), Err(ScopeError::InvalidHost));
        assert_eq!(
            Request::net_listen("0.0.0.0", 0),
            Err(ScopeError::DynamicListen)
        );
        assert_eq!(
            Cidr::parse("::ffff:10.0.0.0/104"),
            Err(ScopeError::InvalidCidr)
        );
    }

    #[test]
    fn child_attenuation_cannot_escalate() {
        let root = test_root("parent");
        let child_root = root.join("child");
        let parent = Policy::new([Rule::allow(read_scope(&root))]);
        let child = Policy::new([
            Rule::allow(read_scope(&child_root)),
            Rule::allow(read_scope(&test_root("outside"))),
        ]);
        let effective = parent.attenuate(&child);

        assert!(
            effective
                .check(&Request::read(child_root.join("ok.js")).unwrap())
                .is_ok()
        );
        assert!(
            effective
                .check(&Request::read(root.join("sibling.js")).unwrap())
                .is_err()
        );
        assert!(
            effective
                .check(&Request::read(test_root("outside/escape.js")).unwrap())
                .is_err()
        );
        assert!(
            parent
                .attenuate(&Policy::default())
                .check(&Request::read(child_root.join("ok.js")).unwrap())
                .is_err()
        );
    }

    #[test]
    fn all_query_requires_global_allow_without_denies() {
        let allow = Policy::allow_all([Capability::Read]);
        assert_eq!(allow.query_all(Capability::Read), Decision::Allowed);

        let root = test_root("private");
        let with_deny = Policy::new([
            Rule::allow(Scope::All(Capability::Read)),
            Rule::deny(read_scope(&root)),
        ]);
        assert_eq!(with_deny.query_all(Capability::Read), Decision::Denied);
    }

    #[test]
    fn malformed_scopes_fail_closed() {
        assert_eq!(NormalizedPath::new(""), Err(ScopeError::Empty));
        assert_eq!(
            NormalizedPath::new("relative/path"),
            Err(ScopeError::InvalidPath)
        );
        assert_eq!(NameScope::prefix(""), Err(ScopeError::Empty));
        assert_eq!(
            NetworkScope::host("", PortRange::exact(80)),
            Err(ScopeError::Empty)
        );
        assert_eq!(PortRange::new(443, 80), Err(ScopeError::InvalidPortRange));
        assert_eq!(
            ImportScope::origin("https://example.test/path"),
            Err(ScopeError::InvalidUrl)
        );
    }
}
