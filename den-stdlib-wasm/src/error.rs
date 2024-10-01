use derive_more::Display;
use rquickjs::class::Trace;
#[derive(Trace)]
#[rquickjs::class]
pub struct Exception {}

#[rquickjs::methods]
impl Exception {
    #[qjs(constructor)]
    pub fn new() {}
}

#[derive(Trace, Debug, Display)]
#[rquickjs::class]
pub struct CompileError {}

impl std::error::Error for CompileError {}

impl Default for CompileError {
    fn default() -> Self {
        Self::new()
    }
}

#[rquickjs::methods]
impl CompileError {
    #[qjs(constructor)]
    pub fn new() -> Self {
        Self {}
    }
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
