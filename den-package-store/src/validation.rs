use std::collections::BTreeSet;

use crate::{NewRelease, PackageStoreError, Result};

pub fn package_name(value: &str) -> Result<()> {
    if value.is_empty() {
        return invalid_package(value, "name is empty");
    }
    if value.len() > 214 {
        return invalid_package(value, "name exceeds 214 bytes");
    }
    if value.contains(['\0', '\\']) || value.trim() != value {
        return invalid_package(
            value,
            "name contains NUL, backslash, or surrounding whitespace",
        );
    }

    let mut parts = value.split('/');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    if parts.next().is_some() {
        return invalid_package(value, "name has too many path segments");
    }
    if let Some(scope) = first.strip_prefix('@') {
        if first.len() == 1 || second.is_none_or(str::is_empty) {
            return invalid_package(value, "scoped names must be `@scope/name`");
        }
        package_segment(value, scope)?;
        package_segment(value, second.unwrap_or_default())
    } else if second.is_some() {
        invalid_package(value, "unscoped names cannot contain `/`")
    } else {
        package_segment(value, first)
    }
}

fn package_segment(full_name: &str, segment: &str) -> Result<()> {
    if segment == "." || segment == ".." {
        return invalid_package(full_name, "`.` and `..` are not package segments");
    }
    let valid = segment.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
    });
    if !valid
        || !segment
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return invalid_package(
            full_name,
            "segments must start with a lowercase letter or digit and contain only lowercase \
             ASCII, digits, `.`, `_`, or `-`",
        );
    }
    Ok(())
}

fn invalid_package<T>(name: &str, reason: &'static str) -> Result<T> {
    Err(PackageStoreError::InvalidPackageName {
        name: name.to_owned(),
        reason,
    })
}

pub fn module_path(value: &str) -> Result<()> {
    if value.is_empty() {
        return invalid_path(value, "path is empty");
    }
    if value.starts_with('/') {
        return invalid_path(value, "path is absolute");
    }
    if value.contains(['\0', '\\']) {
        return invalid_path(value, "path contains NUL or backslash");
    }
    if value
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return invalid_path(value, "path is not canonical POSIX syntax");
    }
    Ok(())
}

fn invalid_path<T>(path: &str, reason: &'static str) -> Result<T> {
    Err(PackageStoreError::InvalidModulePath {
        path: path.to_owned(),
        reason,
    })
}

pub fn export_name(value: &str) -> Result<()> {
    if value == "." {
        return Ok(());
    }
    let Some(path) = value.strip_prefix("./") else {
        return Err(PackageStoreError::InvalidExportName {
            name: value.to_owned(),
        });
    };
    module_path(path).map_err(|_error| {
        PackageStoreError::InvalidExportName {
            name: value.to_owned(),
        }
    })
}

pub fn release(release: &NewRelease) -> Result<()> {
    package_name(&release.package)?;
    let parsed_version = node_semver::Version::parse(&release.version).map_err(|error| {
        PackageStoreError::InvalidVersion {
            version: release.version.clone(),
            reason:  error.to_string(),
        }
    })?;
    if parsed_version.to_string() != release.version {
        return Err(PackageStoreError::InvalidVersion {
            version: release.version.clone(),
            reason:  format!("use canonical form `{parsed_version}`"),
        });
    }

    if release.yanked_reason.as_deref().is_some_and(str::is_empty) {
        return Err(PackageStoreError::InvalidSnapshot(
            "yanked reason is empty".to_owned(),
        ));
    }

    let mut dependencies = BTreeSet::new();
    for dependency in &release.dependencies {
        package_name(&dependency.package)?;
        node_semver::Range::parse(&dependency.requirement).map_err(|error| {
            PackageStoreError::InvalidVersionRange {
                range:  dependency.requirement.clone(),
                reason: error.to_string(),
            }
        })?;
        let identity = dependency
            .alias
            .as_deref()
            .unwrap_or(dependency.package.as_str());
        if dependency.alias.is_some() {
            package_name(identity)?;
        }
        if !dependencies.insert(identity) {
            return Err(PackageStoreError::DuplicateReleaseEntry {
                kind:  "dependency",
                value: identity.to_owned(),
            });
        }
    }

    let mut exports = BTreeSet::new();
    for export in &release.exports {
        export_name(&export.name)?;
        module_path(&export.target)?;
        if !exports.insert(export.name.as_str()) {
            return Err(PackageStoreError::DuplicateReleaseEntry {
                kind:  "export",
                value: export.name.clone(),
            });
        }
    }

    let mut files = BTreeSet::new();
    for file in &release.files {
        module_path(&file.path)?;
        if !files.insert(file.path.as_str()) {
            return Err(PackageStoreError::DuplicateReleaseEntry {
                kind:  "file",
                value: file.path.clone(),
            });
        }
    }
    for export in &release.exports {
        if !files.contains(export.target.as_str()) {
            return Err(PackageStoreError::MissingExportTarget {
                name:   export.name.clone(),
                target: export.target.clone(),
            });
        }
    }
    Ok(())
}
