use std::borrow::Cow;

use den_util::stack;
use oxc_sourcemap::{SourceMap, Token};
use rquickjs::{Context, Runtime, context::EvalOptions};
use url::Url;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn map(source: &str, source_text: &str, tokens: Vec<Token>) -> SourceMap<'static> {
    SourceMap::new(
        None,
        Vec::new(),
        None,
        vec![Cow::Owned(source.to_owned())],
        vec![Some(Cow::Owned(source_text.to_owned()))],
        tokens.into_boxed_slice(),
        None,
    )
}

#[test]
fn prepare_stack_trace_formats_real_structured_frames() -> TestResult {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    context.with(|ctx| -> TestResult {
        stack::install(&ctx)?;
        let source = "function inner() { return new TypeError('boom').stack; }\nfunction outer() \
                      { return inner(); }\nouter();";
        stack::register_source(&ctx, "plain.js", source.to_owned(), std::iter::empty())?;
        let mut options = EvalOptions::default();
        options.filename = Some("plain.js".to_owned());
        let trace: String = ctx.eval_with_options(source, options)?;
        if !trace.starts_with("TypeError: boom\n    at inner (plain.js:1:")
            || !trace.contains("\n    at outer (plain.js:2:")
            || !trace.contains("\n    at <eval> (plain.js:3:")
        {
            return Err(format!("unexpected structured stack:\n{trace}").into());
        }
        Ok(())
    })
}

#[test]
fn source_map_and_utf16_column_are_applied_before_formatting() -> TestResult {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    context.with(|ctx| -> TestResult {
        stack::install(&ctx)?;
        let source = "(() => { const marker = '😀'; return new Error('mapped').stack; })();";
        let mut options = EvalOptions::default();
        options.filename = Some("generated.js".to_owned());
        let raw: String = ctx.eval_with_options(source, options)?;
        let raw_location = stack::first_location(&raw).ok_or("raw stack has no location")?;
        let byte_column = raw_location.column.saturating_sub(1) as usize;
        let utf16_column = source
            .get(..byte_column)
            .ok_or("QuickJS returned an invalid byte column")?
            .encode_utf16()
            .count() as u32;
        let source_map = map(
            "/src/authored.ts",
            "\n\n\n\n  throw new Error('mapped');\n",
            vec![
                Token::new(0, utf16_column, 4, 2, Some(0), None),
                Token::new(0, utf16_column + 1, 8, 9, Some(0), None),
            ],
        );
        stack::register_source(&ctx, "generated.js", source.to_owned(), [source_map])?;
        let mut options = EvalOptions::default();
        options.filename = Some("generated.js".to_owned());
        let trace: String = ctx.eval_with_options(source, options)?;
        if stack::first_location(&trace)
            != Some(stack::Location {
                filename: "/src/authored.ts".to_owned(),
                line:     5,
                column:   3,
            })
        {
            return Err(format!("source map used the wrong column:\n{trace}").into());
        }
        Ok(())
    })
}

#[test]
fn host_errors_dom_exceptions_and_custom_stack_hooks_keep_their_shape() -> TestResult {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    context.with(|ctx| -> TestResult {
        stack::install(&ctx)?;

        let host = stack::error_from_message(&ctx, "from host")?;
        let host_stack: String = host.as_object().get("stack")?;
        if !host_stack.starts_with("Error: from host") {
            return Err(format!("host header was prepared too early: {host_stack}").into());
        }
        let multiline = stack::error_from_message(&ctx, "first\nsecond")?;
        let rendered = stack::format_thrown(&ctx, &multiline.into_object().into_value());
        if rendered.matches("second").count() != 1 {
            return Err(format!("multiline message was duplicated: {rendered}").into());
        }

        let worker_stack = "TypeError: crossed\n    at worker.ts:7:3";
        let rebuilt = stack::error_from_parts(&ctx, Some("TypeError"), "crossed", worker_stack)?;
        let is_type_error: bool = ctx
            .eval::<rquickjs::Function, _>("value => value instanceof TypeError")?
            .call((rebuilt.clone(),))?;
        if !is_type_error || stack::format_thrown(&ctx, &rebuilt) != worker_stack {
            return Err("worker error reconstruction changed its stack or subtype".into());
        }

        let dom: rquickjs::Value = ctx.eval("new DOMException('stopped', 'AbortError')")?;
        let dom_stack = stack::format_thrown(&ctx, &dom);
        if !dom_stack.starts_with("AbortError: stopped") || !dom_stack.contains(" at ") {
            return Err(format!("DOMException lost its stack: {dom_stack}").into());
        }

        let custom: String =
            ctx.eval("Error.prepareStackTrace = () => 'application stack'; new Error('x').stack")?;
        if custom != "application stack" {
            return Err(format!("custom prepareStackTrace was not preserved: {custom}").into());
        }
        Ok(())
    })
}

#[test]
fn source_mapping_url_handles_variants_and_ignores_empty_values() {
    let source = "\
const x = 1;\n//# sourceMappingURL=discarded.map\n//@ sourceMappingURL=active.map\n/*# \
                  sourceMappingURL=active2.map*/\n//# sourceMappingURL= \n";
    assert_eq!(stack::source_mapping_url(source), Some("active2.map"));
    assert_eq!(
        stack::source_mapping_url("//# sourceMappingURL=\n//@ sourceMappingURL=   "),
        None
    );
}

#[test]
fn inline_source_map_only_processes_data_urls() {
    let source_url = Url::parse("https://example.com/main.js").unwrap();
    assert!(stack::inline_source_map("https://example.com/main.js.map", &source_url).is_none());
}

#[test]
fn inline_source_map_decodes_percent_source_maps() {
    let source_url = Url::parse("https://example.com/main.js").unwrap();
    let map = "%7B%22version%22%3A3%2C%22sources%22%3A%5B%22src.ts%22%5D%2C%22names%22%3A%5B%5D%\
               2C%22mappings%22%3A%22%22%2C%22sourcesContent%22%3A%5B%22x%22%5D%7D";
    assert!(
        stack::inline_source_map(&format!("data:application/json,{map}"), &source_url).is_some()
    );
}

#[test]
fn inline_source_map_rejects_non_json_payloads() {
    let source_url = Url::parse("https://example.com/main.js").unwrap();
    assert!(stack::inline_source_map("data:text/plain,not-json", &source_url).is_none());
}

#[test]
fn inline_source_map_decodes_base64_source_maps() {
    let source_url = Url::parse("https://example.com/main.js").unwrap();
    let base64_map = "eyJ2ZXJzaW9uIjozLCJzb3VyY2VzIjpbInNyYy50cyJdLCJuYW1lcyI6W10sIm1hcHBpbmdzIjoiIiwic291cmNlc0NvbnRlbnQiOlsieCJdfQ==";
    assert!(
        stack::inline_source_map(
            &format!("data:application/json;base64,{base64_map}"),
            &source_url
        )
        .is_some()
    );
}

#[test]
fn error_from_parts_applies_mapped_and_unmapped_stack_branches() -> TestResult {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    context.with(|ctx| -> TestResult {
        let mapped = stack::error_from_parts(
            &ctx,
            Some("TypeError"),
            "mapped",
            "TypeError: mapped\n    at test.js:1:1",
        )?;
        assert_eq!(
            stack::format_thrown(&ctx, &mapped),
            "TypeError: mapped\n    at test.js:1:1"
        );

        let unmapped =
            stack::error_from_parts(&ctx, Some("TypeError"), "unmapped", "not a header")?;
        assert_eq!(stack::format_thrown(&ctx, &unmapped), "not a header");

        let empty_stack = stack::error_from_parts(&ctx, Some("TypeError"), "empty", "")?;
        let object = empty_stack
            .as_object()
            .expect("error_from_parts should return an object");
        assert_eq!(object.get::<_, String>("stack")?, "");
        assert_eq!(stack::format_thrown(&ctx, &empty_stack), "TypeError: empty");
        Ok(())
    })
}

#[test]
fn first_location_parses_stacks_and_returns_none_for_unmatched_frames() {
    let stack = "TypeError: boom\n    at test (main.js:12:34)\n    at native";
    assert_eq!(
        stack::first_location(stack),
        Some(stack::Location {
            filename: "main.js".to_owned(),
            line:     12,
            column:   34,
        }),
    );
    assert_eq!(
        stack::first_location("TypeError: boom\n    at frame (main.js:12)"),
        None
    );
    assert_eq!(stack::first_location("TypeError: boom"), None);
}
