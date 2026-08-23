#![cfg(feature = "transpile")]

use std::path::Path;

use derive_more::{Debug, Display, Error};
use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions, CodegenReturn};
use oxc_diagnostics::{NamedSource, OxcDiagnostic};
use oxc_parser::{Parser, ParserReturn};
pub use oxc_sourcemap::OwnedSourceMap as SourceMap;
pub use oxc_span::SourceType as Syntax;

/// Virtual file name used for diagnostics, sourcemap `sources[0]` and the
/// transformer's `source_path`. den transpiles in-memory buffers whose real
/// path is not known at this layer.
const ANONYMOUS_SOURCE: &str = "<anonymous>";

/// How to interpret the top level of a source buffer. Mirrors the swc enum den
/// was built against so call sites stay untouched.
#[derive(Debug, Display, Clone, Copy, PartialEq, Eq)]
pub enum IsModule {
    /// Force module (`true`) or script (`false`) parsing.
    Bool(bool),
    /// Decide from the source: any ESM syntax (or top-level `await`) means
    /// module.
    Unknown,
}

impl IsModule {
    fn apply(self, syntax: Syntax) -> Syntax {
        match self {
            Self::Bool(true) => syntax.with_module(true),
            Self::Bool(false) => syntax.with_script(true),
            Self::Unknown => syntax.with_unambiguous(true),
        }
    }
}

/// Stateless by construction: oxc keeps no source interner, no comment store
/// and no thread-local globals, so there is nothing to carry between calls.
/// `Default` is kept because den's loaders derive `Default` through an
/// `Arc<Self>`.
#[derive(Default)]
pub struct EasyOxcTranspiler;

impl EasyOxcTranspiler {
    pub fn transpile(
        &self, source: &str, syntax: Syntax, is_module: IsModule, emit_sourcemap: bool,
    ) -> Result<(String, Option<SourceMap>), EasyOxcTranspilerError> {
        // The arena, and everything borrowing from it, lives and dies inside this
        // call, so nothing borrowed can escape into the return value.
        // ponytail: fresh arena per call; AllocatorPool if transpile shows up in a
        // profile.
        let allocator = Allocator::new();

        #[cfg_attr(
            not(any(feature = "typescript", feature = "react")),
            allow(
                unused_mut,
                reason = "only the transformer mutates the program in place"
            )
        )]
        let ParserReturn {
            mut program,
            diagnostics,
            panicked,
            ..
        } = Parser::new(&allocator, source, is_module.apply(syntax)).parse();
        // A panicked parse yields an *empty* AST, which would otherwise codegen
        // into an empty module instead of surfacing the syntax error.
        if panicked || diagnostics.has_errors() {
            return Err(EasyOxcTranspilerError::Parse(
                EasyOxcTranspilerError::render(source, diagnostics),
            ));
        }

        #[cfg(any(feature = "typescript", feature = "react"))]
        {
            use oxc_semantic::SemanticBuilder;
            use oxc_transformer::Transformer;

            // `with_enum_eval(true)` is not optional: the TS enum lowering reads
            // pre-computed member values out of `Scoping` and asserts on their
            // presence, emitting wrong reverse mappings when they are missing.
            let semantic = SemanticBuilder::new_compiler()
                .with_enum_eval(true)
                .build(&program);
            if semantic.diagnostics.has_errors() {
                return Err(EasyOxcTranspilerError::Semantic(
                    EasyOxcTranspilerError::render(source, semantic.diagnostics),
                ));
            }
            // `into_scoping` drops the shared borrow of `program` taken above, which
            // is what lets the transformer take it by `&mut`.
            let scoping = semantic.semantic.into_scoping();

            let transformed = Transformer::new(
                &allocator,
                Path::new(ANONYMOUS_SOURCE),
                &Self::transform_options(),
            )
            .build_with_scoping(scoping, &mut program);
            if transformed.diagnostics.has_errors() {
                return Err(EasyOxcTranspilerError::Transform(
                    EasyOxcTranspilerError::render(source, transformed.diagnostics),
                ));
            }
        }

        let CodegenReturn { code, map, .. } = Codegen::new()
            .with_options(CodegenOptions {
                // `source_map_path` is the only sourcemap on/off switch, and it also
                // supplies `sources[0]`.
                source_map_path: emit_sourcemap.then(|| Path::new(ANONYMOUS_SOURCE).to_path_buf()),
                ..CodegenOptions::default()
            })
            .build(&program);

        // The map borrows both the arena and the source text; detach it before the
        // arena drops at the end of this function.
        let source_map = map.map(|map| SourceMap::new(map.into_owned()));
        Ok((code, source_map))
    }

    /// Strip types, keep class fields native, downlevel nothing, and use the
    /// classic JSX runtime: den's resolver has no `react` module, so the
    /// automatic runtime's `import { jsx } from "react/jsx-runtime"` would
    /// fail to load.
    #[cfg(any(feature = "typescript", feature = "react"))]
    fn transform_options() -> oxc_transformer::TransformOptions {
        use oxc_transformer::{JsxOptions, JsxRuntime, TransformOptions};

        TransformOptions {
            jsx: if cfg!(feature = "react") {
                JsxOptions {
                    runtime: JsxRuntime::Classic,
                    ..JsxOptions::enable()
                }
            } else {
                JsxOptions::disable()
            },
            ..TransformOptions::default()
        }
    }
}

#[derive(Debug, Display, Error)]
pub enum EasyOxcTranspilerError {
    #[display("failed to parse source:\n{_0}")]
    Parse(#[error(not(source))] String),
    #[display("failed to analyse source:\n{_0}")]
    Semantic(#[error(not(source))] String),
    #[display("failed to transform source:\n{_0}")]
    Transform(#[error(not(source))] String),
}

impl EasyOxcTranspilerError {
    /// oxc diagnostics carry spans only, so they are worthless once the source
    /// text is gone — render them eagerly instead of storing the pair.
    fn render(source: &str, diagnostics: impl IntoIterator<Item = OxcDiagnostic>) -> String {
        diagnostics
            .into_iter()
            .map(|diagnostic| {
                diagnostic.render_with_source_code(NamedSource::new(ANONYMOUS_SOURCE, source))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Maps the extensions den registers with its loaders onto an oxc `SourceType`.
/// Module-vs-script is deliberately not decided here: `IsModule` overrides it
/// at transpile time.
pub fn infer_transpile_syntax_by_extension(extension: &str) -> Option<Syntax> {
    match extension {
        "js" | "mjs" => Syntax::from_extension(extension).ok(),
        // oxc has no `mjsx` extension, so build the JSX source type by hand.
        "jsx" | "mjsx" => cfg!(feature = "react").then(Syntax::jsx),
        "ts" => cfg!(feature = "typescript").then(Syntax::ts),
        "tsx" => cfg!(all(feature = "typescript", feature = "react")).then(Syntax::tsx),
        _ => None,
    }
}

#[derive(Display, Error, Debug)]
pub enum InferTranspileSyntaxError {
    InvalidExtension,
}

pub const fn get_best_transpiling() -> &'static str {
    match (cfg!(feature = "typescript"), cfg!(feature = "react")) {
        (false, false) => "js",
        (false, true) => "jsx",
        (true, false) => "ts",
        (true, true) => "tsx",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn transpile(
        source: &str, extension: &str, is_module: IsModule,
    ) -> (String, Option<SourceMap>) {
        let syntax = infer_transpile_syntax_by_extension(extension)
            .unwrap_or_else(|| panic!("extension {extension} must be inferable in this build"));
        EasyOxcTranspiler
            .transpile(source, syntax, is_module, true)
            .expect("transpile must succeed")
    }

    #[test]
    fn plain_javascript_passes_through() {
        let (code, _) = transpile("const add = (a, b) => a + b;", "js", IsModule::Bool(false));
        assert!(code.contains("const add = (a, b) => a + b;"), "{code}");
    }

    #[test]
    fn unknown_module_kind_resolves_both_ways() {
        let (script, _) = transpile("var a = 1", "js", IsModule::Unknown);
        assert!(!script.contains("export"), "{script}");
        let (module, _) = transpile("export const a = 1", "js", IsModule::Unknown);
        assert!(module.contains("export"), "{module}");
    }

    /// den's REPL transpiles with `IsModule::Unknown`, so top-level `await` has
    /// to upgrade the buffer to ESM rather than fail to parse.
    #[test]
    fn top_level_await_is_accepted_when_module_kind_is_unknown() {
        let (code, _) = transpile(
            "await import(`./x.js`)",
            get_best_transpiling(),
            IsModule::Unknown,
        );
        assert!(code.contains("import("), "{code}");
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn typescript_types_are_stripped() {
        let (code, _) = transpile(
            "const x: number = 1; interface Foo { a: string } export {};",
            "ts",
            IsModule::Bool(true),
        );
        assert!(!code.contains("interface"), "{code}");
        assert!(!code.contains(": number"), "{code}");
    }

    /// Regression guard for `SemanticBuilder::with_enum_eval(true)`: without it
    /// the enum lowering trips a `debug_assert!` and emits wrong reverse
    /// mappings.
    #[cfg(feature = "typescript")]
    #[test]
    fn typescript_enum_lowers_to_a_runtime_object() {
        let (code, _) = transpile(
            "enum E { A, B } namespace N { export const q = 1 }",
            "ts",
            IsModule::Bool(false),
        );
        assert!(code.contains(r#"E[E["A"]"#), "{code}");
    }

    /// Equivalent of swc's `native_class_properties = true`.
    #[cfg(feature = "typescript")]
    #[test]
    fn class_fields_stay_native() {
        let (code, _) = transpile(
            "class A { x = 1; declare y: number; z!: string; }",
            "ts",
            IsModule::Bool(true),
        );
        assert!(code.contains("x = 1"), "{code}");
        assert!(!code.contains("declare"), "{code}");
    }

    /// Guards against `JsxOptions::default()`, whose automatic runtime emits an
    /// `import ... from "react/jsx-runtime"` that den cannot resolve.
    #[cfg(feature = "react")]
    #[test]
    fn jsx_becomes_create_element_calls() {
        let (code, _) = transpile(
            "const a = <div x={1}>hi</div>;",
            "jsx",
            IsModule::Bool(true),
        );
        assert!(code.contains("React.createElement"), "{code}");
        assert!(!code.contains("jsx-runtime"), "{code}");
    }

    #[cfg(all(feature = "typescript", feature = "react"))]
    #[test]
    fn tsx_becomes_create_element_calls() {
        let (code, _) = transpile(
            "const a = <div x={1 as number}>hi</div>;",
            "tsx",
            IsModule::Unknown,
        );
        assert!(code.contains("React.createElement"), "{code}");
    }

    /// The map borrows the arena and the source text; this fails to compile,
    /// then fails at runtime, if `into_owned` is dropped.
    #[test]
    fn sourcemap_is_emitted_and_detached_from_the_arena() {
        let (_, source_map) = transpile("const x = 1; export {};", "js", IsModule::Bool(true));
        let json = source_map.expect("sourcemap requested").to_json_string();
        assert!(json.contains("\"mappings\""), "{json}");
        assert!(json.contains(ANONYMOUS_SOURCE), "{json}");
    }

    /// den's only real call pattern.
    #[test]
    fn no_sourcemap_when_not_requested() {
        let syntax = infer_transpile_syntax_by_extension("js").unwrap();
        let (_, source_map) = EasyOxcTranspiler
            .transpile("const x = 1;", syntax, IsModule::Bool(true), false)
            .expect("transpile must succeed");
        assert!(source_map.is_none());
    }

    #[test]
    fn syntax_error_renders_with_a_source_snippet() {
        let syntax = infer_transpile_syntax_by_extension("js").unwrap();
        let error = EasyOxcTranspiler
            .transpile("const = ;", syntax, IsModule::Unknown, false)
            .expect_err("malformed source must not transpile")
            .to_string();
        assert!(error.contains("failed to parse source"), "{error}");
        assert!(error.contains(ANONYMOUS_SOURCE), "{error}");
    }

    #[test]
    fn extension_inference_matches_the_active_features() {
        for extension in ["js", "mjs"] {
            assert!(
                infer_transpile_syntax_by_extension(extension).is_some(),
                "{extension}"
            );
        }
        for extension in ["json", "wasm", "", "d.ts"] {
            assert!(
                infer_transpile_syntax_by_extension(extension).is_none(),
                "{extension}"
            );
        }
        for extension in ["jsx", "mjsx"] {
            assert_eq!(
                infer_transpile_syntax_by_extension(extension).is_some(),
                cfg!(feature = "react"),
                "{extension}"
            );
        }
        assert_eq!(
            infer_transpile_syntax_by_extension("ts").is_some(),
            cfg!(feature = "typescript")
        );
        assert_eq!(
            infer_transpile_syntax_by_extension("tsx").is_some(),
            cfg!(all(feature = "typescript", feature = "react"))
        );
        // Whatever the REPL picks has to be inferable in this build.
        assert!(infer_transpile_syntax_by_extension(get_best_transpiling()).is_some());
    }

    /// den's loaders hold the transpiler in an `Arc` shared across tokio tasks.
    #[test]
    fn transpiler_is_shareable_across_threads() {
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<EasyOxcTranspiler>();
        assert_send_sync::<SourceMap>();

        let shared = Arc::new(EasyOxcTranspiler);
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    let syntax =
                        infer_transpile_syntax_by_extension(get_best_transpiling()).unwrap();
                    let source = format!("export const w{index} = {index};");
                    shared
                        .transpile(&source, syntax, IsModule::Bool(true), true)
                        .expect("transpile must succeed")
                        .0
                })
            })
            .collect();
        for (index, worker) in workers.into_iter().enumerate() {
            assert!(worker.join().unwrap().contains(&format!("w{index}")));
        }
    }
}
