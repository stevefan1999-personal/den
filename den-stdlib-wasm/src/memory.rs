use either::Either;
use rquickjs::{class::Trace, prelude::*, ArrayBuffer, Ctx, Object};

#[derive(Trace)]
#[rquickjs::class]
pub struct Memory {
    #[qjs(skip_trace)]
    initial: usize,
    #[qjs(skip_trace)]
    maximum: Option<usize>,
    #[qjs(skip_trace)]
    shared:  bool,
}

#[rquickjs::methods]
impl Memory {
    #[qjs(constructor)]
    pub fn new<'js>(opts: Object<'js>) -> rquickjs::Result<Self> {
        let initial = opts.get::<_, usize>("initial")?;
        let maximum = opts.get::<_, Option<usize>>("maximum")?;
        let shared = opts.get::<_, bool>("shared").unwrap_or(false);

        Ok(Self {
            initial,
            maximum,
            shared,
        })
    }

    #[qjs(get, enumerable)]
    pub fn exports<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<ArrayBuffer<'js>> {
        todo!()
    }
}
