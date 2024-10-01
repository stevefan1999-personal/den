use derive_more::derive::{Deref, DerefMut, From, Into};
use indexmap::indexmap;
use rquickjs::{class::Trace, prelude::*, ArrayBuffer, Ctx, Exception, Result, Value};
use typed_builder::TypedBuilder;
use wasmtime::AsContextMut;

use crate::MyUserData;

#[derive(Trace, Clone, Deref, DerefMut, From, Into)]
#[rquickjs::class]
pub struct Memory<'js> {
    #[qjs(skip_trace)]
    pub(crate) inner: wasmtime::Memory,

    #[deref(ignore)]
    #[deref_mut(ignore)]
    pub(crate) store: crate::store::Store<'js>,
}

#[derive(Clone, TypedBuilder)]
pub struct MemoryDescriptor {
    initial: u64,
    maximum: Option<u64>,
    shared:  Option<bool>,
}

impl<'js> FromJs<'js> for MemoryDescriptor {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if let Some(this) = value.into_object() {
            Ok(Self {
                initial: this.get("initial")?,
                maximum: this.get("maximum")?,
                shared:  this.get("shared")?,
            })
        } else {
            Err(ctx.throw("not an object".into_js(ctx)?))
        }
    }
}

impl<'js> IntoJs<'js> for MemoryDescriptor {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        indexmap! {
            "initial" => self.initial.into_js(ctx)?,
            "maximum" => self.maximum.into_js(ctx)?,
            "shared" => self.shared.into_js(ctx)?,
        }
        .into_js(ctx)
    }
}

#[rquickjs::methods]
impl<'js> Memory<'js> {
    #[qjs(constructor)]
    pub fn new(
        opts: MemoryDescriptor,
        Opt(store): Opt<crate::store::Store<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let ty = wasmtime::MemoryTypeBuilder::default()
            .min(opts.initial)
            .max(opts.maximum)
            .shared(opts.shared.unwrap_or(false))
            .build()
            .map_err(|x| {
                Exception::throw_internal(
                    &ctx,
                    &format!("wasm linker build memory type error: {}", x),
                )
            })?;

        let store = store.unwrap_or(ctx.userdata::<MyUserData>().unwrap().store.clone());

        Ok(Self {
            inner: {
                let mut store = store.lock().unwrap();

                wasmtime::Memory::new(store.as_context_mut(), ty).map_err(|x| {
                    Exception::throw_internal(&ctx, &format!("wasm linker memory new error: {}", x))
                })?
            },
            store,
        })
    }

    #[qjs(get, enumerable)]
    pub fn buffer(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        let _data = self
            .inner
            .data_mut(self.store.lock().unwrap().as_context_mut());
        Err(ctx.throw("TODO".into_js(&ctx)?))

        // let val = unsafe {
        //     let val = qjs::JS_NewArrayBuffer(
        //         ctx.as_raw().as_mut(),
        //         data.as_mut_ptr(),
        //         data.len().try_into().unwrap(),
        //         None, // No need for deallocation
        //         ptr::null_mut(), // No opaque
        //         1, // True = Shared
        //     );
        //     if qjs::JS_VALUE_GET_NORM_TAG(val) == qjs::JS_TAG_EXCEPTION {
        //         return Err(rquickjs::Error::Exception);
        //     }

        //     Value::from_js_value(ctx.clone(), val)
        // };

        // let buffer =
        // ArrayBuffer::from_object(Object::from_value(val)?).unwrap();

        // Ok(buffer)
    }

    pub fn grow(&self, delta: u64, ctx: Ctx<'_>) -> Result<()> {
        self.inner
            .grow(self.store.lock().unwrap().as_context_mut(), delta)
            .map_err(|x| {
                Exception::throw_internal(&ctx, &format!("wasm linker memory grow error: {}", x))
            })?;
        Ok(())
    }
}
