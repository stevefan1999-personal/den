#[cfg(any(feature = "typescript", feature = "react"))]
use std::path::Path;

use derive_more::{Debug, Display, Error};
use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenReturn};
use oxc_diagnostics::{NamedSource, OxcDiagnostic};
use oxc_parser::{Parser, ParserReturn};
pub use oxc_span::SourceType;

/// Virtual file name used for diagnostics and the transformer's `source_path`.
/// den transpiles in-memory buffers whose real path is not known at this layer.
const ANONYMOUS_SOURCE: &str = "<anonymous>";

pub fn transpile(source: &str, source_type: SourceType) -> Result<String, EasyOxcTranspilerError> {
    // ponytail: fresh arena per call; use AllocatorPool if profiling justifies it.
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
    } = Parser::new(&allocator, source, source_type).parse();
    // A panicked parse yields an empty AST, which would otherwise codegen into
    // an empty module instead of surfacing the syntax error.
    if panicked || diagnostics.has_errors() {
        return Err(EasyOxcTranspilerError::Parse(
            EasyOxcTranspilerError::render(source, diagnostics),
        ));
    }

    #[cfg(any(feature = "typescript", feature = "react"))]
    {
        use oxc_semantic::SemanticBuilder;
        use oxc_transformer::Transformer;

        // TS enum lowering requires pre-computed member values in `Scoping`.
        let semantic = SemanticBuilder::new_compiler()
            .with_enum_eval(true)
            .build(&program);
        if semantic.diagnostics.has_errors() {
            return Err(EasyOxcTranspilerError::Semantic(
                EasyOxcTranspilerError::render(source, semantic.diagnostics),
            ));
        }
        let scoping = semantic.semantic.into_scoping();

        let transformed = Transformer::new(
            &allocator,
            Path::new(ANONYMOUS_SOURCE),
            &transform_options(),
        )
        .build_with_scoping(scoping, &mut program);
        if transformed.diagnostics.has_errors() {
            return Err(EasyOxcTranspilerError::Transform(
                EasyOxcTranspilerError::render(source, transformed.diagnostics),
            ));
        }
    }

    let CodegenReturn { code, .. } = Codegen::new().build(&program);
    Ok(code)
}

/// Strip types, downlevel nothing, and use the classic JSX runtime: den's
/// resolver has no `react/jsx-runtime` module.
#[cfg(any(feature = "typescript", feature = "react"))]
fn transform_options() -> oxc_transformer::TransformOptions {
    use oxc_transformer::{JsxOptions, TransformOptions};

    #[cfg(feature = "react")]
    let jsx = JsxOptions {
        runtime: oxc_transformer::JsxRuntime::Classic,
        ..JsxOptions::enable()
    };
    #[cfg(not(feature = "react"))]
    let jsx = JsxOptions::disable();

    TransformOptions {
        jsx,
        ..TransformOptions::default()
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

/// Maps the extensions den registers with its loaders onto an Oxc source type.
pub fn infer_transpile_syntax_by_extension(extension: &str) -> Option<SourceType> {
    match extension {
        "js" | "mjs" => SourceType::from_extension(extension).ok(),
        // oxc has no `mjsx` extension, so build the JSX source type by hand.
        #[cfg(feature = "react")]
        "jsx" | "mjsx" => Some(SourceType::jsx()),
        #[cfg(feature = "typescript")]
        "ts" => Some(SourceType::ts()),
        #[cfg(all(feature = "typescript", feature = "react"))]
        "tsx" => Some(SourceType::tsx()),
        _ => None,
    }
}

pub const fn get_best_transpiling() -> &'static str {
    #[cfg(not(any(feature = "typescript", feature = "react")))]
    {
        "js"
    }
    #[cfg(all(not(feature = "typescript"), feature = "react"))]
    {
        "jsx"
    }
    #[cfg(all(feature = "typescript", not(feature = "react")))]
    {
        "ts"
    }
    #[cfg(all(feature = "typescript", feature = "react"))]
    {
        "tsx"
    }
}
