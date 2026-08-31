//! `WebAssembly.Tag` — the identity of an exception kind.

use indexmap::indexmap;
use rquickjs::{
    Array, Ctx, Exception, FromJs, IntoJs as _, JsLifetime, Object, Result, Value, class::Trace,
};
use wasmtime::ValType;

use crate::memory::{DescriptorObject as _, ValueTypeName};

/// A `TagType`: the parameter types an exception thrown with this tag carries.
#[expect(
    clippy::module_name_repetitions,
    reason = "WebAssembly names this descriptor TagType"
)]
#[derive(Clone, Debug)]
pub struct TagType {
    parameters: Vec<ValType>,
}

impl<'js> FromJs<'js> for TagType {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let object = Object::descriptor(ctx, value, "the tag type")?;
        let parameters: Vec<String> = object.required(ctx, "parameters")?;
        parameters
            .iter()
            .map(|name| ValueTypeName::value(ctx, name, "tag parameter type"))
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
    #[qjs(skip_trace)]
    pub(crate) inner: ::wasmtime::Tag,
}

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

    pub(crate) fn parameters(&self, ctx: &Ctx<'_>) -> Result<Vec<ValType>> {
        crate::store::Store::from_ctx(ctx)?.with_mut(ctx, |store| {
            Ok(self.inner.ty(&*store).ty().params().collect())
        })
    }
}

#[rquickjs::methods]
impl Tag {
    #[qjs(constructor)]
    pub fn new(ty: TagType, ctx: Ctx<'_>) -> Result<Self> { Self::allocate(ty, &ctx) }

    /// The js-types reflection method: `{ parameters: ["i32", …] }`.
    #[qjs(rename = "type")]
    pub fn tag_type<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        let parameters = Array::new(ctx.clone())?;
        for (index, ty) in self.parameters(&ctx)?.iter().enumerate() {
            let name = ValueTypeName::get(ty).ok_or_else(|| {
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
#[path = "../tests/unit/tag.rs"]
mod tests;
