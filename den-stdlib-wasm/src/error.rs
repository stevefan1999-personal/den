use rquickjs::class::Trace;
#[derive(Trace)]
#[rquickjs::class]
pub struct Exception {}

#[rquickjs::methods]
impl Exception {
    #[qjs(constructor)]
    pub fn new() {}
}

#[derive(Trace)]
#[rquickjs::class]
pub struct CompileError {}

#[rquickjs::methods]
impl CompileError {
    #[qjs(constructor)]
    pub fn new() {}
}

#[derive(Trace)]
#[rquickjs::class]
pub struct LinkError {}

#[rquickjs::methods]
impl LinkError {
    #[qjs(constructor)]
    pub fn new() {}
}

#[derive(Trace)]
#[rquickjs::class]
pub struct RuntimeError {}

#[rquickjs::methods]
impl RuntimeError {
    #[qjs(constructor)]
    pub fn new() {}
}
