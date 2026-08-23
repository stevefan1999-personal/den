//! `WebAssembly.Tag` — the identity of an exception kind.

use indexmap::indexmap;
use rquickjs::{
    Array, Ctx, Exception, FromJs, IntoJs, JsLifetime, Object, Result, Value, class::Trace,
};

use crate::{
    backend,
    memory::{DescriptorObject, ValueTypeName},
};

/// A `TagType`: the parameter types an exception thrown with this tag carries.
#[derive(Clone, Debug)]
pub struct TagType {
    parameters: Vec<backend::ValType>,
}

impl<'js> FromJs<'js> for TagType {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let object = Object::descriptor(ctx, value, "the tag type")?;
        let parameters: Vec<String> = object.required(ctx, "parameters")?;
        parameters
            .iter()
            .map(|name| {
                ValueTypeName::resolve(ctx, name, ValueTypeName::GLOBAL, "tag parameter type")
            })
            .collect::<Result<Vec<_>>>()
            .map(|parameters| Self { parameters })
    }
}

/// A tag, identified by the object itself.
///
/// The spec identifies tags by their address in the store and keeps a
/// per-agent address-to-object cache so that the two always agree. den has no
/// such cache yet, so `Exception.is`/`getArg` compare `Tag` objects instead —
/// which gives the same answer for every tag JS can currently get hold of,
/// since tags cannot yet be imported from or exported to a module.
#[derive(Trace, JsLifetime, Clone, Debug)]
#[rquickjs::class]
pub struct Tag {
    /// The store-allocated tag; the parameter types are read back from it
    /// rather than cached, so the store stays the single source of truth.
    #[cfg(feature = "wasmtime")]
    #[qjs(skip_trace)]
    pub(crate) inner: ::wasmtime::Tag,
}

#[cfg(feature = "wasmtime")]
impl Tag {
    /// `tag_alloc(store, parameters -> « »)`.
    fn allocate(ty: TagType, ctx: &Ctx<'_>) -> Result<Self> {
        crate::store::Store::from_ctx(ctx)?.with_mut(ctx, |store| {
            let signature = ::wasmtime::TagType::new(::wasmtime::FuncType::new(
                store.engine(),
                ty.parameters,
                [],
            ));
            ::wasmtime::Tag::new(&mut *store, &signature)
                .map(|inner| Self { inner })
                .map_err(|error| Exception::throw_type(ctx, &format!("cannot create tag: {error}")))
        })
    }

    pub(crate) fn parameters(&self, ctx: &Ctx<'_>) -> Result<Vec<backend::ValType>> {
        crate::store::Store::from_ctx(ctx)?.with_mut(ctx, |store| {
            Ok(self.inner.ty(&*store).ty().params().collect())
        })
    }
}

/// wasmi implements neither the exception-handling proposal nor any tag type,
/// so there is nothing to allocate and nothing a tag could ever be used for.
#[cfg(not(feature = "wasmtime"))]
impl Tag {
    fn allocate(ty: TagType, ctx: &Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(
            ctx,
            &format!(
                "a WebAssembly.Tag with {} parameters cannot be created: tags are not supported \
                 by the {} backend of this build",
                ty.parameters.len(),
                backend::NAME
            ),
        ))
    }

    /// Unreachable in practice: no `Tag` can be constructed on this backend.
    pub(crate) fn parameters(&self, _ctx: &Ctx<'_>) -> Result<Vec<backend::ValType>> {
        Ok(Vec::new())
    }
}

#[rquickjs::methods]
impl Tag {
    #[qjs(constructor)]
    pub fn new(ty: TagType, ctx: Ctx<'_>) -> Result<Self> {
        Self::allocate(ty, &ctx)
    }

    /// The js-types reflection method: `{ parameters: ["i32", …] }`.
    #[qjs(rename = "type")]
    pub fn tag_type<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        let parameters = Array::new(ctx.clone())?;
        for (index, ty) in self.parameters(&ctx)?.iter().enumerate() {
            let name = backend::val_type_name(ty).ok_or_else(|| {
                Exception::throw_type(&ctx, "this tag parameter type has no JS name")
            })?;
            parameters.set(index, name)?;
        }
        indexmap! {
            "parameters" => parameters,
        }
        .into_js(&ctx)?
        .into_object()
        .ok_or_else(|| Exception::throw_type(&ctx, "tag type is not an object"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::testing::{js, pending_error_name, with_wasm_context};

    fn tag(ctx: &Ctx<'_>, ty: &str) -> core::result::Result<Tag, String> {
        TagType::from_js(ctx, js(ctx, ty))
            .and_then(|ty| Tag::new(ty, ctx.clone()))
            .map_err(|_| pending_error_name(ctx))
    }

    #[test]
    fn a_tag_reports_the_parameter_types_it_was_created_with() {
        with_wasm_context(|ctx| {
            let created = tag(ctx, "({ parameters: ['i32', 'f64'] })");
            if !backend::SUPPORTS_TAGS {
                assert_eq!(created.unwrap_err(), "TypeError");
                return;
            }
            let ty = created.expect("tag").tag_type(ctx.clone()).expect("type()");
            ctx.globals().set("type", ty).expect("bind");
            let parameters: String = ctx
                .eval("type.parameters.join(',')")
                .expect("parameters is an array");
            assert_eq!(parameters, "i32,f64");
        })
    }

    #[test]
    fn a_tag_type_with_an_unknown_parameter_type_is_a_type_error() {
        with_wasm_context(|ctx| {
            for ty in [
                "({ parameters: ['nope'] })",
                "({ parameters: ['v128'] })",
                "({})",
                "(1)",
            ] {
                assert_eq!(tag(ctx, ty).unwrap_err(), "TypeError", "{ty}");
            }
        })
    }
}
