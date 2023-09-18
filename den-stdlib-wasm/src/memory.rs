use rquickjs::class::Trace;
#[derive(Trace)]
#[rquickjs::class]
pub struct Memory {}

#[rquickjs::methods]
impl Memory {
    #[qjs(constructor)]
    pub fn new() {}
}
