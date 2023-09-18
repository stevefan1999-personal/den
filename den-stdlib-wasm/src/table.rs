use rquickjs::class::Trace;
#[derive(Trace)]
#[rquickjs::class]
pub struct Table {}

#[rquickjs::methods]
impl Table {
    #[qjs(constructor)]
    pub fn new() {}
}
