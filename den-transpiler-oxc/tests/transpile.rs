use den_transpiler_oxc::{
    EasyOxcTranspilerError, SourceType, get_best_transpiling, infer_transpile_syntax_by_extension,
    transpile,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn source_type(extension: &str) -> Result<SourceType, Box<dyn std::error::Error>> {
    infer_transpile_syntax_by_extension(extension)
        .ok_or_else(|| format!("extension {extension} must be inferable in this build").into())
}

fn transpiled(source: &str, source_type: SourceType) -> Result<String, EasyOxcTranspilerError> {
    transpile(source, source_type)
}

#[test]
fn plain_javascript_output() -> TestResult {
    insta::assert_snapshot!(transpiled(
        include_str!("fixtures/plain.js"),
        source_type("js")?.with_script(true),
    )?);
    Ok(())
}

#[test]
fn unknown_module_kind_resolves_both_ways() -> TestResult {
    let script = transpiled(
        include_str!("fixtures/unambiguous_script.js"),
        source_type("js")?.with_unambiguous(true),
    )?;
    let module = transpiled(
        include_str!("fixtures/unambiguous_module.js"),
        source_type("js")?.with_unambiguous(true),
    )?;
    insta::assert_snapshot!(format!("script:\n{script}\nmodule:\n{module}"));
    Ok(())
}

#[test]
fn top_level_await_is_accepted_when_module_kind_is_unknown() -> TestResult {
    insta::assert_snapshot!(transpiled(
        include_str!("fixtures/top_level_await.js"),
        source_type(get_best_transpiling())?.with_unambiguous(true),
    )?);
    Ok(())
}

#[cfg(feature = "typescript")]
#[test]
fn typescript_types_are_stripped() -> TestResult {
    insta::assert_snapshot!(transpiled(
        include_str!("fixtures/types.ts"),
        source_type("ts")?.with_module(true),
    )?);
    Ok(())
}

#[cfg(feature = "typescript")]
#[test]
fn typescript_enum_lowers_to_a_runtime_object() -> TestResult {
    insta::assert_snapshot!(transpiled(
        include_str!("fixtures/enum.ts"),
        source_type("ts")?.with_script(true),
    )?);
    Ok(())
}

#[cfg(feature = "typescript")]
#[test]
fn class_fields_stay_native() -> TestResult {
    insta::assert_snapshot!(transpiled(
        include_str!("fixtures/class_fields.ts"),
        source_type("ts")?.with_module(true),
    )?);
    Ok(())
}

#[cfg(feature = "react")]
#[test]
fn jsx_becomes_create_element_calls() -> TestResult {
    insta::assert_snapshot!(transpiled(
        include_str!("fixtures/component.jsx"),
        source_type("jsx")?.with_module(true),
    )?);
    Ok(())
}

#[cfg(all(feature = "typescript", feature = "react"))]
#[test]
fn tsx_becomes_create_element_calls() -> TestResult {
    insta::assert_snapshot!(transpiled(
        include_str!("fixtures/component.tsx"),
        source_type("tsx")?.with_unambiguous(true),
    )?);
    Ok(())
}

#[test]
fn syntax_error_diagnostic() -> TestResult {
    let result = transpile(
        include_str!("fixtures/syntax_error.js"),
        source_type("js")?.with_unambiguous(true),
    );
    let Err(error) = result else {
        return Err("malformed source transpiled successfully".into());
    };
    assert!(matches!(&error, EasyOxcTranspilerError::Parse(_)));
    insta::assert_snapshot!(error);
    Ok(())
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
    assert!(infer_transpile_syntax_by_extension(get_best_transpiling()).is_some());
}
