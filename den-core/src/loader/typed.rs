//! Synthetic modules for `import … with { type: "json" | "text" | "bytes" }`.
//!
//! Both file and HTTP loaders share this so a type attribute means the same
//! thing regardless of where the body came from. Missing `type` is not handled
//! here: that stays on the JS/TS transpile path.

use rquickjs::{Ctx, Error, Module, Result, loader::ImportAttributes, module::Declared};

/// The WinterTC / txiki import-attribute types den knows how to load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImportKind {
    Json,
    Text,
    Bytes,
}

/// Classify `attributes`, or `None` when the JS/TS path should run.
///
/// An unknown `type` is a loading error rather than a fall-through: the
/// attribute is an explicit request, and guessing script would hide it.
pub(crate) fn import_kind<'js>(
    name: &str,
    attributes: Option<&ImportAttributes<'js>>,
) -> Result<Option<ImportKind>> {
    let Some(attributes) = attributes else {
        return Ok(None);
    };
    let Some(kind) = attributes.get_type()? else {
        return Ok(None);
    };
    match kind.as_str() {
        "json" => Ok(Some(ImportKind::Json)),
        "text" => Ok(Some(ImportKind::Text)),
        "bytes" => Ok(Some(ImportKind::Bytes)),
        other => {
            Err(Error::new_loading_message(
                name,
                format!("unsupported module type: '{other}'"),
            ))
        }
    }
}

/// Declare a module whose default export is `bytes` interpreted as `kind`.
pub(crate) fn declare_import_kind<'js>(
    ctx: &Ctx<'js>,
    name: &str,
    bytes: &[u8],
    kind: ImportKind,
) -> Result<Module<'js, Declared>> {
    let source = match kind {
        ImportKind::Json => json_module_source(name, bytes)?,
        ImportKind::Text => text_module_source(name, bytes)?,
        ImportKind::Bytes => bytes_module_source(bytes),
    };
    Module::declare(ctx.clone(), name, source)
}

/// `export default <json>;` — the body must already be valid JSON so that
/// evaluating it as a JS expression is `JSON.parse` of the same text.
fn json_module_source(name: &str, bytes: &[u8]) -> Result<String> {
    let text = utf8_body(bytes)?;
    serde_json::from_str::<serde_json::Value>(text)
        .map_err(|error| Error::new_loading_message(name, format!("invalid JSON: {error}")))?;
    Ok(format!("export default {text};"))
}

/// `export default ${JSON.stringify(text)};`
fn text_module_source(name: &str, bytes: &[u8]) -> Result<String> {
    let text = utf8_body(bytes)?;
    let literal = serde_json::to_string(text)
        .map_err(|error| Error::new_loading_message(name, error.to_string()))?;
    Ok(format!("export default {literal};"))
}

/// `export default Uint8Array.from([..]);` — self-contained, so a bytes import
/// does not depend on `atob` being installed as a global.
fn bytes_module_source(bytes: &[u8]) -> String {
    let body = bytes
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("export default Uint8Array.from([{body}]);")
}

fn utf8_body(bytes: &[u8]) -> Result<&str> {
    let text = std::str::from_utf8(bytes)?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(text))
}

#[cfg(test)]
mod tests {
    use super::{bytes_module_source, json_module_source, text_module_source};

    #[test]
    fn json_module_embeds_the_validated_text() {
        assert_eq!(
            json_module_source("x.json", br#"{"foo":1}"#).unwrap(),
            r#"export default {"foo":1};"#
        );
    }

    #[test]
    fn json_module_rejects_invalid_json() {
        assert!(json_module_source("bad.json", b"{").is_err());
    }

    #[test]
    fn text_module_json_stringifies_the_body() {
        assert_eq!(
            text_module_source("hello.txt", b"a \"quote\"").unwrap(),
            r#"export default "a \"quote\"";"#
        );
    }

    #[test]
    fn bytes_module_lists_each_byte() {
        assert_eq!(
            bytes_module_source(&[0, 1, 255]),
            "export default Uint8Array.from([0,1,255]);"
        );
    }
}
