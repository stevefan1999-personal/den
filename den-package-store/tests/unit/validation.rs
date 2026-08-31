use super::{export_name, module_path, package_name, release};
use crate::{
    BlobDigest, DependencyKind, NewDependency, NewExport, NewPackageFile, NewRelease,
    PackageStoreError, RegistryId,
};

fn invalid_name(name: &str) -> bool {
    matches!(
        package_name(name),
        Err(PackageStoreError::InvalidPackageName { .. })
    )
}

fn invalid_path(path: &str) -> bool {
    matches!(
        module_path(path),
        Err(PackageStoreError::InvalidModulePath { .. })
    )
}

#[test]
fn package_names_reject_empty_scoped_and_uppercase_forms() {
    assert!(package_name("left-pad").is_ok());
    assert!(package_name("@scope/pkg").is_ok());
    assert!(package_name("a".repeat(214).as_str()).is_ok());
    assert!(invalid_name(""));
    assert!(invalid_name("A"));
    assert!(invalid_name(" has-space"));
    assert!(invalid_name("has space"));
    assert!(invalid_name("slash\\name"));
    assert!(invalid_name("nul\0name"));
    assert!(invalid_name("@"));
    assert!(invalid_name("@scope"));
    assert!(invalid_name("@scope/"));
    assert!(invalid_name("unscoped/name"));
    assert!(invalid_name("@a/b/c"));
    assert!(invalid_name(".hidden"));
    assert!(invalid_name("..dot"));
    assert!(invalid_name(&"a".repeat(215)));
}

#[test]
fn module_paths_must_be_canonical_posix_relatives() {
    assert!(module_path("index.js").is_ok());
    assert!(module_path("lib/main.js").is_ok());
    assert!(invalid_path(""));
    assert!(invalid_path("/abs.js"));
    assert!(invalid_path("a\\b.js"));
    assert!(invalid_path("a\0b.js"));
    assert!(invalid_path("a//b.js"));
    assert!(invalid_path("a/./b.js"));
    assert!(invalid_path("a/../b.js"));
}

#[test]
fn export_names_are_dot_or_relative_module_paths() {
    assert!(export_name(".").is_ok());
    assert!(export_name("./lib.js").is_ok());
    assert!(matches!(
        export_name("lib.js"),
        Err(PackageStoreError::InvalidExportName { .. })
    ));
    assert!(matches!(
        export_name("./../escape.js"),
        Err(PackageStoreError::InvalidExportName { .. })
    ));
}

#[test]
fn release_validation_covers_version_yank_duplicates_and_missing_targets() {
    let registry = RegistryId(1);
    let digest = BlobDigest::for_bytes(b"body");
    let mut valid = NewRelease::new(registry, "pkg", "1.0.0");
    valid.files.push(NewPackageFile {
        path:       "index.js".into(),
        blob:       digest,
        media_type: Some("text/javascript".into()),
        mode:       0o644,
    });
    valid.exports.push(NewExport {
        name:   ".".into(),
        target: "index.js".into(),
    });
    valid.dependencies.push(NewDependency {
        kind:               DependencyKind::Normal,
        target_registry_id: Some(registry),
        package:            "left-pad".into(),
        requirement:        "^1.0.0".into(),
        alias:              Some("pad".into()),
    });
    assert!(release(&valid).is_ok());

    let mut bad_version = NewRelease::new(registry, "pkg", "01.0.0");
    assert!(matches!(
        release(&bad_version),
        Err(PackageStoreError::InvalidVersion { .. })
    ));
    bad_version.version = "not-a-version".into();
    assert!(matches!(
        release(&bad_version),
        Err(PackageStoreError::InvalidVersion { .. })
    ));

    let mut yanked = NewRelease::new(registry, "pkg", "1.0.0");
    yanked.yanked_reason = Some(String::new());
    assert!(matches!(
        release(&yanked),
        Err(PackageStoreError::InvalidSnapshot(_))
    ));

    let mut duplicate = valid.clone();
    duplicate.dependencies.push(NewDependency {
        kind:               DependencyKind::Peer,
        target_registry_id: None,
        package:            "other".into(),
        requirement:        "1.0.0".into(),
        alias:              Some("pad".into()),
    });
    assert!(matches!(
      release(&duplicate),
      Err(PackageStoreError::DuplicateReleaseEntry { kind, .. }) if kind == "dependency"
    ));

    let mut duplicate_export = valid.clone();
    duplicate_export.exports.push(NewExport {
        name:   ".".into(),
        target: "index.js".into(),
    });
    assert!(matches!(
      release(&duplicate_export),
      Err(PackageStoreError::DuplicateReleaseEntry { kind, .. }) if kind == "export"
    ));

    let mut duplicate_file = valid.clone();
    duplicate_file.files.push(NewPackageFile {
        path:       "index.js".into(),
        blob:       digest,
        media_type: None,
        mode:       0o644,
    });
    assert!(matches!(
      release(&duplicate_file),
      Err(PackageStoreError::DuplicateReleaseEntry { kind, .. }) if kind == "file"
    ));

    let mut missing = valid;
    missing.exports.push(NewExport {
        name:   "./missing".into(),
        target: "missing.js".into(),
    });
    assert!(matches!(
        release(&missing),
        Err(PackageStoreError::MissingExportTarget { .. })
    ));
}
