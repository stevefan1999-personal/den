use rquickjs::class::Trace;
#[derive(Trace)]
#[rquickjs::class]
pub struct Tag {}

#[rquickjs::methods]
impl Tag {
    #[qjs(constructor)]
    pub fn new() {}
}
