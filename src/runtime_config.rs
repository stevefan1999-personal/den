use std::{collections::BTreeSet, path::PathBuf};

use color_eyre::eyre::{Result, WrapErr as _, eyre};
use den_config::Config;
#[cfg(feature = "package-store")]
use den_config::RegistryConfig;
use den_core::{EngineBuilder, engine::Engine};

struct PackageRequirement {
    specifier:   String,
    #[cfg(feature = "package-store")]
    registry:    String,
    #[cfg(feature = "package-store")]
    package:     String,
    #[cfg(feature = "package-store")]
    requirement: String,
}

pub async fn build_engine(config: Option<&Config>, argv: Vec<String>) -> Result<Engine> {
    let mut builder = EngineBuilder::new().argv(argv);
    let Some(config) = config else {
        return Ok(builder.build().await);
    };
    validate_runtime_config(config)?;

    builder = builder.policy(config.policy()?);
    if let Some(budgets) = config.budgets() {
        if let Some(bytes) = budgets.stack_bytes {
            builder = builder.max_stack_size(bytes);
        }
        if let Some(bytes) = budgets.heap_bytes {
            builder = builder
                .heap_limit(usize::try_from(bytes).wrap_err("heapBytes does not fit this target")?);
        }
    }

    let packages = package_requirements(config)?;
    if let Some((json, base)) = import_map(config, &packages)? {
        builder = builder.import_map(&json, base)?;
    }

    #[cfg(feature = "package-store")]
    if !packages.is_empty() {
        builder = builder.package_modules(package_snapshot(config, &packages).await?);
    }
    #[cfg(not(feature = "package-store"))]
    if !packages.is_empty() {
        return Err(eyre!(
            "package dependencies require a den build with the `package-store` feature"
        ));
    }

    Ok(builder.build().await)
}

fn validate_runtime_config(config: &Config) -> Result<()> {
    if config.offline() == Some(true) {
        return Err(eyre!("`offline` is not supported by the den runtime"));
    }
    if config.frozen() == Some(true) {
        return Err(eyre!("`frozen` is not supported by the den runtime"));
    }
    if config.reload() == Some(true) {
        return Err(eyre!("`reload` is not supported by the den runtime"));
    }
    if config.env_files().is_some_and(|files| !files.is_empty()) {
        return Err(eyre!("`envFiles` is not supported by the den runtime"));
    }
    if let Some(budgets) = config.budgets() {
        if budgets.timeout_ms.is_some() {
            return Err(eyre!(
                "`budgets.timeoutMs` is not supported by the den runtime"
            ));
        }
        if budgets.max_workers.is_some() {
            return Err(eyre!(
                "`budgets.maxWorkers` is not supported by the den runtime"
            ));
        }
    }
    Ok(())
}

pub async fn run_preloads(engine: &Engine, config: Option<&Config>) -> Result<()> {
    if let Some(preloads) = config.and_then(Config::preloads) {
        for preload in preloads {
            engine
                .run_file(preload.clone())
                .await
                .wrap_err_with(|| format!("failed to preload `{}`", preload.display()))?;
        }
    }
    Ok(())
}

fn package_requirements(config: &Config) -> Result<Vec<PackageRequirement>> {
    let registries = config
        .registries()
        .map(|registries| {
            registries
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    config
        .dependencies()
        .into_iter()
        .flatten()
        .filter_map(|(specifier, target)| {
            let (registry, package) = target.split_once(':')?;
            if registries.contains(registry) {
                return Some(parse_package_requirement(specifier, registry, package));
            }
            (!matches!(registry, "file" | "link" | "http" | "https")).then(|| {
                Err(eyre!(
                    "registry `{registry}` for dependency `{specifier}` is not configured"
                ))
            })
        })
        .collect()
}

fn parse_package_requirement(
    specifier: &str, registry: &str, target: &str,
) -> Result<PackageRequirement> {
    let (package, requirement) = target
        .rsplit_once('@')
        .filter(|(package, _requirement)| !package.is_empty())
        .unwrap_or((target, "*"));
    if package.is_empty() || requirement.is_empty() {
        return Err(eyre!(
            "invalid package dependency `{specifier}`: `{registry}:{target}`"
        ));
    }
    Ok(PackageRequirement {
        specifier:                                     specifier.to_owned(),
        #[cfg(feature = "package-store")]
        registry:                                      registry.to_owned(),
        #[cfg(feature = "package-store")]
        package:                                       package.to_owned(),
        #[cfg(feature = "package-store")]
        requirement:                                   requirement.to_owned(),
    })
}

fn import_map(
    config: &Config, packages: &[PackageRequirement],
) -> Result<Option<(String, PathBuf)>> {
    let mut map = if let Some(path) = config.import_map() {
        serde_json::from_str(
            &std::fs::read_to_string(path)
                .wrap_err_with(|| format!("failed to read import map `{}`", path.display()))?,
        )
        .wrap_err_with(|| format!("failed to parse import map `{}`", path.display()))?
    } else {
        serde_json::json!({})
    };
    let base = config.import_map().map_or_else(
        || {
            config
                .root()
                .map(PathBuf::from)
                .map_or_else(std::env::current_dir, Ok)
        },
        |path| {
            Ok(path
                .parent()
                .map_or_else(|| PathBuf::from("."), PathBuf::from))
        },
    )?;
    let package_specifiers = packages
        .iter()
        .map(|package| package.specifier.as_str())
        .collect::<BTreeSet<_>>();
    let mut configured = config.imports().cloned().unwrap_or_default();
    for (specifier, target) in config.dependencies().into_iter().flatten() {
        if !package_specifiers.contains(specifier.as_str()) {
            configured
                .entry(specifier.clone())
                .or_insert_with(|| target.strip_prefix("link:").unwrap_or(target).to_owned());
        }
    }
    if configured.is_empty() && config.import_map().is_none() {
        return Ok(None);
    }

    let object = map
        .as_object_mut()
        .ok_or_else(|| eyre!("the import map root must be an object"))?;
    let imports = object
        .entry("imports")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| eyre!("the import map `imports` value must be an object"))?;
    imports.extend(
        configured
            .into_iter()
            .map(|(name, target)| (name, serde_json::Value::String(target))),
    );
    Ok(Some((serde_json::to_string(&map)?, base)))
}

#[cfg(feature = "package-store")]
async fn package_snapshot(
    config: &Config, packages: &[PackageRequirement],
) -> Result<std::sync::Arc<den_package_store::PackageModuleSnapshot>> {
    use den_package_store::{PackageStore, RootRequirement};

    let path = config
        .package_store()
        .ok_or_else(|| eyre!("package dependencies require `packageStore` in den.json"))?;
    let store = PackageStore::open(path)
        .await
        .wrap_err_with(|| format!("failed to open package store `{}`", path.display()))?;
    let registries = config
        .registries()
        .ok_or_else(|| eyre!("package dependencies require `registries` in den.json"))?;
    let mut roots = Vec::with_capacity(packages.len());
    for package in packages {
        let registry = registries
            .get(&package.registry)
            .ok_or_else(|| eyre!("registry `{}` is not configured", package.registry))?;
        let url = match registry {
            RegistryConfig::Url(url) => url,
            RegistryConfig::Detailed(options) => &options.url,
        };
        let id = store
            .registry_id(&package.registry, url)
            .await?
            .ok_or_else(|| {
                eyre!(
                    "registry `{}` ({url}) is not present in the package store",
                    package.registry
                )
            })?;
        roots.push(RootRequirement::aliased(
            &package.specifier,
            id,
            &package.package,
            &package.requirement,
        ));
    }
    let solved = store.repository_snapshot().await?.solve(&roots)?;
    Ok(std::sync::Arc::new(store.hydrate_modules(&solved).await?))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{build_engine, run_preloads};

    #[tokio::test]
    async fn config_applies_imports_policy_budgets_and_preloads()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        fs::write(temp.path().join("answer.js"), "export const answer = 42")?;
        fs::write(
            temp.path().join("preload.js"),
            "globalThis.preloaded = true",
        )?;
        let config_path = temp.path().join("den.json");
        fs::write(
            &config_path,
            r#"{
                "imports": { "answer": "./answer.js" },
                "permissions": { "read": ["."] },
                "budgets": { "stackBytes": 65536, "heapBytes": 16777216 },
                "preloads": ["preload.js"]
            }"#,
        )?;
        let config = den_config::Config::load(config_path)?;
        let engine = build_engine(Some(&config), vec!["den".into()]).await?;
        run_preloads(&engine, Some(&config)).await?;

        assert_eq!(
            engine
                .eval::<i32>("await import('answer').then(module => module.answer)")
                .await?,
            42
        );
        assert!(engine.eval::<bool>("globalThis.preloaded").await?);
        assert_eq!(engine.policy().await.rules().count(), 1);
        assert!(
            engine
                .eval::<()>("(function recurse() { recurse(); })()")
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn unsupported_runtime_controls_and_unknown_registries_fail_early()
    -> Result<(), Box<dyn std::error::Error>> {
        for (name, field) in [
            ("offline", r#""offline": true"#),
            ("frozen", r#""frozen": true"#),
            ("reload", r#""reload": true"#),
            ("envFiles", r#""envFiles": [".env"]"#),
            ("budgets.timeoutMs", r#""budgets": { "timeoutMs": 1 }"#),
            ("budgets.maxWorkers", r#""budgets": { "maxWorkers": 1 }"#),
        ] {
            let temp = tempdir()?;
            let path = temp.path().join("den.json");
            fs::write(&path, format!("{{ {field} }}"))?;
            let config = den_config::Config::load(path)?;
            let error = match build_engine(Some(&config), vec!["den".into()]).await {
                Ok(_engine) => panic!("{name} should be rejected"),
                Err(error) => error,
            };
            assert!(error.to_string().contains(name), "{error:#}");
        }

        let temp = tempdir()?;
        let path = temp.path().join("den.json");
        fs::write(
            &path,
            r#"{ "dependencies": { "alias": "jsr:real-package@1" } }"#,
        )?;
        let config = den_config::Config::load(path)?;
        let error = match build_engine(Some(&config), vec!["den".into()]).await {
            Ok(_engine) => panic!("an unknown registry should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("registry `jsr`"), "{error:#}");
        Ok(())
    }

    #[cfg(feature = "package-store")]
    #[tokio::test]
    async fn config_hydrates_aliased_package_dependencies() -> Result<(), Box<dyn std::error::Error>>
    {
        use den_package_store::{NewExport, NewPackageFile, NewRelease, PackageStore};

        let temp = tempdir()?;
        let store_path = temp.path().join("packages.db");
        let store = PackageStore::create(&store_path).await?;
        let registry = store.add_registry("jsr", "https://jsr.example/").await?;
        let source = b"export default 42";
        let digest = store.insert_blob(source).await?;
        let mut release = NewRelease::new(registry, "real-package", "1.0.0");
        release.exports.push(NewExport {
            name:   ".".to_owned(),
            target: "main.js".to_owned(),
        });
        release.files.push(NewPackageFile {
            path:       "main.js".to_owned(),
            blob:       digest,
            media_type: Some("text/javascript".to_owned()),
            mode:       0o644,
        });
        store.insert_release(&release).await?;
        drop(store);

        let config_path = temp.path().join("den.json");
        fs::write(
            &config_path,
            serde_json::to_vec(&serde_json::json!({
                "packageStore": store_path,
                "registries": { "jsr": "https://jsr.example/" },
                "dependencies": { "alias": "jsr:real-package@1.0.0" }
            }))?,
        )?;
        let config = den_config::Config::load(config_path)?;
        let engine = build_engine(Some(&config), vec!["den".into()]).await?;
        assert_eq!(
            engine
                .eval::<i32>("await import('alias').then(module => module.default)")
                .await?,
            42
        );
        Ok(())
    }
}
