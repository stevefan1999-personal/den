use std::sync::Arc;

use den_package_store::PackageModuleSnapshot;
#[cfg(feature = "transpile")]
use den_transpiler_oxc::{infer_transpile_syntax_by_extension, transpile_with_source_map};
use rquickjs::{
    Ctx, Error, Exception, Module, Result,
    loader::{ImportAttributes, Loader, Resolver},
    module::Declared,
};
use url::Url;

use crate::loader::typed::{declare_import_kind, import_kind};

/// Synchronous package resolution over one immutable, pre-hydrated snapshot.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::module_name_repetitions,
    reason = "qualified name distinguishes this resolver at engine call sites"
)]
pub struct PackageResolver {
    snapshot: Option<Arc<PackageModuleSnapshot>>,
}

impl PackageResolver {
    #[must_use]
    pub const fn new(snapshot: Arc<PackageModuleSnapshot>) -> Self {
        Self {
            snapshot: Some(snapshot),
        }
    }

    #[must_use]
    pub(crate) const fn optional(snapshot: Option<Arc<PackageModuleSnapshot>>) -> Self {
        Self { snapshot }
    }
}

impl Resolver for PackageResolver {
    fn resolve<'js>(
        &mut self, ctx: &Ctx<'js>, base: &str, name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| Error::new_resolving(base, name))?;
        match snapshot.resolve_if_claimed(base, name) {
            None => Err(Error::new_resolving(base, name)),
            Some(Ok(resolved)) => Ok(resolved),
            Some(Err(error)) => Err(Exception::throw_type(ctx, &error.to_string())),
        }
    }
}

/// Synchronous package loading over one immutable, pre-hydrated snapshot.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::module_name_repetitions,
    reason = "qualified name distinguishes this loader at engine call sites"
)]
pub struct PackageLoader {
    snapshot: Option<Arc<PackageModuleSnapshot>>,
}

impl PackageLoader {
    #[must_use]
    pub const fn new(snapshot: Arc<PackageModuleSnapshot>) -> Self {
        Self {
            snapshot: Some(snapshot),
        }
    }

    #[must_use]
    pub(crate) const fn optional(snapshot: Option<Arc<PackageModuleSnapshot>>) -> Self {
        Self { snapshot }
    }
}

impl Loader for PackageLoader {
    fn load<'js>(
        &mut self, ctx: &Ctx<'js>, name: &str, attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| Error::new_loading(name))?;
        let module = snapshot
            .module(name)
            .ok_or_else(|| Error::new_loading(name))?;
        if let Some(kind) = import_kind(name, attributes.as_ref())? {
            return declare_import_kind(ctx, name, module.bytes(), kind);
        }

        let extension = script_extension(module.path(), module.media_type())
            .ok_or_else(|| Error::new_loading_message(name, "unsupported package module type"))?;
        let authored = std::str::from_utf8(module.bytes())?;
        let authored_map = load_source_map(snapshot, name, authored);

        #[cfg(feature = "transpile")]
        {
            let source_type = infer_transpile_syntax_by_extension(extension)
                .ok_or_else(|| Error::new_loading_message(name, "unsupported script syntax"))?
                .with_module(true);
            let output = transpile_with_source_map(authored, source_type, name)
                .map_err(|error| Error::new_loading_message(name, error.to_string()))?;
            let mut maps = vec![output.source_map.into_inner()];
            maps.extend(authored_map);
            den_util::stack::register_source(ctx, name, output.code.clone(), maps)?;
            Module::declare(ctx.clone(), name, output.code)
        }
        #[cfg(not(feature = "transpile"))]
        {
            if !matches!(extension, "js" | "mjs") {
                return Err(Error::new_loading_message(
                    name,
                    "package module requires den-core's `transpile` feature",
                ));
            }
            let source = authored.to_owned();
            den_util::stack::register_source(ctx, name, source.clone(), authored_map)?;
            Module::declare(ctx.clone(), name, source)
        }
    }
}

fn script_extension<'a>(path: &'a str, media_type: Option<&str>) -> Option<&'a str> {
    let media_extension = media_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .and_then(|value| {
            if value.eq_ignore_ascii_case("text/typescript")
                || value.eq_ignore_ascii_case("application/typescript")
            {
                Some("ts")
            } else if value.eq_ignore_ascii_case("text/tsx")
                || value.eq_ignore_ascii_case("application/tsx")
            {
                Some("tsx")
            } else if value.eq_ignore_ascii_case("text/jsx")
                || value.eq_ignore_ascii_case("application/jsx")
            {
                Some("jsx")
            } else if value.eq_ignore_ascii_case("text/javascript")
                || value.eq_ignore_ascii_case("application/javascript")
            {
                Some("js")
            } else {
                None
            }
        });
    media_extension.or_else(|| {
        let extension = path.rsplit_once('.')?.1;
        matches!(extension, "js" | "mjs" | "jsx" | "mjsx" | "ts" | "tsx").then_some(extension)
    })
}

fn load_source_map(
    snapshot: &PackageModuleSnapshot, name: &str, source: &str,
) -> Option<oxc_sourcemap::SourceMap<'static>> {
    let mapping = den_util::stack::source_mapping_url(source)?;
    let source_url = Url::parse(name).ok()?;
    if mapping.starts_with("data:") {
        return den_util::stack::inline_source_map(mapping, &source_url);
    }
    let specifier = if mapping.starts_with(['.', '/']) || mapping.starts_with("den-pkg:") {
        mapping.to_owned()
    } else {
        format!("./{mapping}")
    };
    let map_name = snapshot.resolve(name, &specifier).ok()?;
    let map_url = Url::parse(&map_name).ok()?;
    let map = snapshot.module(&map_name)?;
    let json = std::str::from_utf8(map.bytes()).ok()?;
    den_util::stack::parse_source_map(json, &map_url)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use den_package_store::{
        NewExport, NewPackageFile, NewRelease, PackageModuleSnapshot, PackageResolutionError,
        PackageStore, RootRequirement,
    };
    #[cfg(feature = "transpile")]
    use rquickjs::Module;
    use rquickjs::{
        Context, Ctx, Error, Runtime, embed,
        loader::{Bundle, ImportAttributes, Resolver},
    };

    #[cfg(feature = "transpile")]
    use super::PackageLoader;
    use super::PackageResolver;
    use crate::{
        EngineBuilder,
        resolver::import_map::{ImportMap, ImportMapResolver},
    };

    const PACKAGE_MAIN: &str =
        "den-pkg://module/jsr/https:%2F%2Fjsr.example%2F/@scope%2Fapp/1.0.0/src/main.ts";
    static SHADOW_BUNDLE: Bundle = {
        const _: &[u8] = include_bytes!("../tests/fixtures/engine/embedded/answer.js");
        embed! {
            "@scope/app": "tests/fixtures/engine/embedded/answer.js",
            "den-pkg://module/jsr/https:%2F%2Fjsr.example%2F/@scope%2Fapp/1.0.0/src/main.ts":
                "tests/fixtures/engine/embedded/answer.js",
        }
    };

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    #[cfg(feature = "transpile")]
    #[tokio::test]
    async fn imports_js_tsx_and_typed_data_without_io() -> TestResult {
        let snapshot = Arc::new(fixture().await?);
        let runtime = Runtime::new()?;
        runtime.set_loader(
            PackageResolver::new(snapshot.clone()),
            PackageLoader::new(snapshot),
        );
        let context = Context::full(&runtime)?;
        context.with(|ctx| -> rquickjs::Result<()> {
            den_util::stack::install(&ctx)?;
            Module::evaluate(
                ctx,
                "snapshot-test",
                r#"
                    import result from "@scope/app";
                    if (result !== "42:tsx:json:text:3") throw new Error(result);
                "#,
            )?
            .finish()
        })?;
        Ok(())
    }

    #[cfg(feature = "transpile")]
    #[tokio::test]
    async fn package_modules_precede_embedded_bundle_entries() -> TestResult {
        let snapshot = Arc::new(fixture().await?);
        assert_eq!(snapshot.resolve("entry", "@scope/app")?, PACKAGE_MAIN);
        let engine = EngineBuilder::new()
            .bundle(SHADOW_BUNDLE)
            .package_modules(snapshot)
            .build()
            .await;
        let value = engine
            .eval::<String>("(await import('@scope/app')).default")
            .await?;
        assert_eq!(value, "42:tsx:json:text:3");
        Ok(())
    }

    #[tokio::test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "assertions verify strict package boundaries and exports"
    )]
    async fn resolver_rejects_traversal_and_missing_exports() -> TestResult {
        let snapshot = fixture().await?;
        let root = snapshot.resolve("entry", "@scope/app")?;
        assert!(matches!(
            snapshot.resolve(&root, "../../outside.js"),
            Err(PackageResolutionError::PackageTraversal { .. })
        ));
        assert!(matches!(
            snapshot.resolve("entry", "@scope/app/private"),
            Err(PackageResolutionError::MissingExport { .. })
        ));
        Ok(())
    }

    #[derive(Clone)]
    struct FallbackResolver(Arc<AtomicBool>);

    impl Resolver for FallbackResolver {
        fn resolve<'js>(
            &mut self, _ctx: &Ctx<'js>, _base: &str, _name: &str,
            _attributes: Option<ImportAttributes<'js>>,
        ) -> rquickjs::Result<String> {
            self.0.store(true, Ordering::Release);
            Ok("fallback.js".to_owned())
        }
    }

    #[tokio::test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "assertions verify terminal package errors in a resolver tuple"
    )]
    async fn claimed_package_errors_do_not_fall_through() -> TestResult {
        let snapshot = Arc::new(fixture().await?);
        let fallback_called = Arc::new(AtomicBool::new(false));
        let mut resolver = (
            PackageResolver::new(snapshot),
            FallbackResolver(fallback_called.clone()),
        );
        let runtime = Runtime::new()?;
        let context = Context::full(&runtime)?;
        context.with(|ctx| {
            assert_eq!(
                resolver.resolve(&ctx, "entry", "local-file", None)?,
                "fallback.js"
            );
            fallback_called.store(false, Ordering::Release);
            assert!(matches!(
                resolver.resolve(&ctx, "entry", "@scope/app/private", None),
                Err(Error::Exception)
            ));
            assert!(!fallback_called.load(Ordering::Acquire));
            let _ = ctx.catch();
            Ok::<_, Error>(())
        })?;
        Ok(())
    }

    #[tokio::test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "assertions verify import maps cannot bypass solved package edges"
    )]
    async fn package_imports_ignore_application_import_maps() -> TestResult {
        let snapshot = Arc::new(fixture().await?);
        let package_base = snapshot.resolve("entry", "@scope/app")?;
        let fallback_called = Arc::new(AtomicBool::new(false));
        let mut resolver = (
            ImportMapResolver,
            PackageResolver::new(snapshot),
            FallbackResolver(fallback_called.clone()),
        );
        let runtime = Runtime::new()?;
        let context = Context::full(&runtime)?;
        let map = ImportMap::parse(
            r#"{"imports":{"undeclared":"./filesystem-bypass.js"}}"#,
            &std::env::current_dir()?,
        )?;
        context.with(|ctx| {
            assert!(ctx.store_userdata(map).is_ok());
            assert!(matches!(
                resolver.resolve(&ctx, &package_base, "undeclared", None),
                Err(Error::Exception)
            ));
            assert!(!fallback_called.load(Ordering::Acquire));
            let _ = ctx.catch();
        });
        Ok(())
    }

    async fn fixture() -> TestResult<PackageModuleSnapshot> {
        let store = PackageStore::open_in_memory().await?;
        let registry = store.add_registry("jsr", "https://jsr.example/").await?;
        let files: [(&str, &[u8], &str); 7] = [
            (
                "src/main.ts",
                br#"
                    import answer from "./child.js";
                    import view from "./view.tsx";
                    import data from "./data.json" with { type: "json" };
                    import text from "./message.txt" with { type: "text" };
                    import bytes from "./payload.bin" with { type: "bytes" };
                    const typed: number = answer;
                    export default `${typed}:${view.type}:${data.kind}:${text}:${bytes.length}`;
                    //# sourceMappingURL=./main.ts.map
                "#,
                "text/typescript",
            ),
            ("src/child.js", b"export default 42", "text/javascript"),
            (
                "src/view.tsx",
                b"
                    const React = { createElement: (type: string) => ({ type }) };
                    export default <tsx />;
                ",
                "text/tsx",
            ),
            ("src/data.json", br#"{"kind":"json"}"#, "application/json"),
            ("src/message.txt", b"text", "text/plain"),
            ("src/payload.bin", &[1, 2, 3], "application/octet-stream"),
            (
                "src/main.ts.map",
                br#"{"version":3,"sources":["main.original.ts"],"names":[],"mappings":""}"#,
                "application/json",
            ),
        ];
        let mut release = NewRelease::new(registry, "@scope/app", "1.0.0");
        release.exports.push(NewExport {
            name:   ".".to_owned(),
            target: "src/main.ts".to_owned(),
        });
        for (path, bytes, media_type) in files {
            let digest = store.insert_blob(bytes).await?;
            release.files.push(NewPackageFile {
                path:       path.to_owned(),
                blob:       digest,
                media_type: Some(media_type.to_owned()),
                mode:       0o644,
            });
        }
        store.insert_release(&release).await?;
        let solved = store
            .repository_snapshot()
            .await?
            .solve(&[RootRequirement::new(registry, "@scope/app", "1.0.0")])?;
        Ok(store.hydrate_modules(&solved).await?)
    }
}
