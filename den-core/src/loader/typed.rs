//! Synthetic modules for `import … with { type: "json" | "text" | "bytes" }`.
//!
//! Both file and HTTP loaders share this so a type attribute means the same
//! thing regardless of where the body came from. Missing `type` is not handled
//! here: that stays on the JS/TS transpile path.

use rquickjs::{
    CatchResultExt as _, Ctx, Error, Module, Result, loader::ImportAttributes, module::Declared,
};

/// The WinterTC / txiki import-attribute types den knows how to load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportKind {
    Json,
    Text,
    Bytes,
}

/// Classify `attributes`, or `None` when the JS/TS path should run.
///
/// An unknown `type` is a loading error rather than a fall-through: the
/// attribute is an explicit request, and guessing script would hide it.
pub fn import_kind(
    name: &str, attributes: Option<&ImportAttributes<'_>>,
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
pub fn declare_import_kind<'js>(
    ctx: &Ctx<'js>, name: &str, bytes: &[u8], kind: ImportKind,
) -> Result<Module<'js, Declared>> {
    let source = match kind {
        ImportKind::Json => json_module_source(ctx, name, bytes)?,
        ImportKind::Text => text_module_source(ctx, name, bytes)?,
        ImportKind::Bytes => bytes_module_source(bytes),
    };
    den_util::stack::register_source(ctx, name, source.clone(), std::iter::empty())?;
    Module::declare(ctx.clone(), name, source)
}

/// `export default <json>;` — the body must already be valid JSON so that
/// evaluating it as a JS expression is `JSON.parse` of the same text.
fn json_module_source(ctx: &Ctx<'_>, name: &str, bytes: &[u8]) -> Result<String> {
    let text = utf8_body(bytes)?;
    ctx.json_parse(text)
        .catch(ctx)
        .map_err(|error| Error::new_loading_message(name, format!("invalid JSON: {error}")))?;
    Ok(format!("export default {text};"))
}

/// `export default ${JSON.stringify(text)};`
fn text_module_source(ctx: &Ctx<'_>, name: &str, bytes: &[u8]) -> Result<String> {
    let text = utf8_body(bytes)?;
    let literal = ctx
        .json_stringify(text)?
        .ok_or_else(|| Error::new_loading_message(name, "cannot stringify text"))?
        .to_string()?;
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
