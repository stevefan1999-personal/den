use rquickjs::class::Trace;
#[derive(Trace)]
#[rquickjs::class]
pub struct Global {}

#[rquickjs::methods]
impl Global {
    #[qjs(constructor)]
    pub fn new() {}
}
