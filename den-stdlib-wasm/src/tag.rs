use rquickjs::{class::Trace, JsLifetime};
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct Tag {}

#[rquickjs::methods]
impl Tag {
    #[qjs(constructor)]
    pub fn new() {}
}
