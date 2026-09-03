use std::{
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
};

use den_capabilities::{
    Capability, ImportScope, NameScope, NetworkScope, NetworkTarget, NormalizedPath, Policy,
    PortRange, Request, ResourceName, Rule, Scope, ScopeError,
};

fn abs(path: &str) -> PathBuf {
    let trimmed = path.trim_start_matches('/');
    if cfg!(windows) {
        Path::new(r"C:\").join(trimmed.replace('/', "\\"))
    } else {
        PathBuf::from(path)
    }
}

#[test]
fn capability_display_uses_kebab_case() {
    assert_eq!(Capability::Read.to_string(), "read");
    assert_eq!(Capability::NetConnect.to_string(), "net-connect");
    assert_eq!(Capability::NetListen.to_string(), "net-listen");
    assert_eq!(Capability::Secrets.to_string(), "secrets");
}

#[test]
fn normalized_paths_are_absolute_and_do_not_cross_component_boundaries() {
    let root = NormalizedPath::new(abs("/srv/app")).expect("absolute");
    let nested = NormalizedPath::new(abs("/srv/app/src/../lib")).expect("dot-dot");
    assert!(root.contains(&nested));
    assert!(!root.contains(&NormalizedPath::new(abs("/srv/application")).expect("sibling")));
    assert_eq!(NormalizedPath::new(""), Err(ScopeError::Empty));
    assert_eq!(
        NormalizedPath::new("relative"),
        Err(ScopeError::InvalidPath)
    );
    let escaped = if cfg!(windows) {
        PathBuf::from(r"C:\..\escape")
    } else {
        PathBuf::from("/../escape")
    };
    assert_eq!(NormalizedPath::new(escaped), Err(ScopeError::InvalidPath));
}

#[test]
fn empty_policies_deny_every_request() {
    let empty = Policy::default();
    assert!(
        empty
            .check(&Request::read(abs("/tmp/file")).expect("path"))
            .is_err()
    );
    assert!(
        empty
            .check(&Request::write(abs("/tmp/file")).expect("path"))
            .is_err()
    );
    assert!(empty.check(&Request::env("PATH").expect("name")).is_err());
    assert!(
        empty
            .check(&Request::net_connect("example.test", 443).expect("target"))
            .is_err()
    );
    assert!(
        empty
            .check(&Request::import("https://example.test/a.js").expect("url"))
            .is_err()
    );
}

#[test]
fn deny_rules_win_over_matching_allows() {
    let policy = Policy::new([
        Rule::allow(Scope::All(Capability::Read)),
        Rule::deny(Scope::Read(
            NormalizedPath::new(abs("/secret")).expect("path"),
        )),
    ]);
    assert!(
        policy
            .check(&Request::read(abs("/tmp")).expect("path"))
            .is_ok()
    );
    assert!(
        policy
            .check(&Request::read(abs("/secret/key")).expect("path"))
            .is_err()
    );
}

#[test]
fn env_and_name_scopes_match_exact_names_only() {
    let policy = Policy::new([
        Rule::allow(Scope::Env(NameScope::exact("DEN_TOKEN").expect("exact"))),
        Rule::deny(Scope::Env(NameScope::exact("DEN_SECRET").expect("exact"))),
    ]);
    assert!(
        policy
            .check(&Request::env("DEN_TOKEN").expect("name"))
            .is_ok()
    );
    assert!(
        policy
            .check(&Request::env("DEN_TOKENS").expect("name"))
            .is_err()
    );
    assert!(
        policy
            .check(&Request::env("DEN_SECRET").expect("name"))
            .is_err()
    );
    assert!(policy.check(&Request::env("PATH").expect("name")).is_err());
    assert_eq!(ResourceName::new(""), Err(ScopeError::Empty));
    assert_eq!(ResourceName::new("A=B"), Err(ScopeError::InvalidName));
}

#[test]
fn network_scopes_match_hosts_and_reject_invalid_port_ranges() {
    assert!(PortRange::new(80, 443).expect("range").contains(443));
    assert_eq!(PortRange::new(443, 80), Err(ScopeError::InvalidPortRange));
    let scope = NetworkScope::host("example.test", PortRange::exact(443)).expect("host");
    let policy = Policy::new([Rule::allow(Scope::NetConnect(scope))]);
    assert!(
        policy
            .check(&Request::net_connect("example.test", 443).expect("target"))
            .is_ok()
    );
    assert!(
        policy
            .check(&Request::net_connect("example.test", 80).expect("target"))
            .is_err()
    );
    assert_eq!(
        Request::net_listen("0.0.0.0", 80),
        Err(ScopeError::DynamicListen)
    );
    assert_eq!(
        Request::net_listen("127.0.0.1", 0),
        Err(ScopeError::DynamicListen)
    );
    let cidr = NetworkScope::cidr("10.0.0.0/8", PortRange::exact(443)).expect("cidr");
    let cidr_policy = Policy::new([Rule::allow(Scope::NetConnect(cidr))]);
    let resolved = NetworkTarget::with_resolved("svc.internal", 443, [IpAddr::V4(Ipv4Addr::new(
        10, 1, 2, 3,
    ))])
    .expect("resolved");
    assert!(cidr_policy.check(&Request::NetConnect(resolved)).is_ok());
}

#[test]
fn import_scopes_match_exact_and_prefix() {
    let exact = Policy::new([Rule::allow(Scope::Import(
        ImportScope::exact("https://example.test/mod.js").expect("url"),
    ))]);
    assert!(
        exact
            .check(&Request::import("https://example.test/mod.js").expect("url"))
            .is_ok()
    );
    assert!(
        exact
            .check(&Request::import("https://example.test/other.js").expect("url"))
            .is_err()
    );

    let prefix = Policy::new([Rule::allow(Scope::Import(
        ImportScope::prefix("https://example.test/lib/").expect("prefix"),
    ))]);
    assert!(
        prefix
            .check(&Request::import("https://example.test/lib/a.js").expect("url"))
            .is_ok()
    );
    assert!(
        prefix
            .check(&Request::import("https://example.test/app.js").expect("url"))
            .is_err()
    );
}
