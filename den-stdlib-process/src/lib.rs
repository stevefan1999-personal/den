//! Process, environment, spawn, signals and DNS lookup (`den:process`).
//!
//! The same object is installed as `globalThis.process` (via `evaluate_def`)
//! and exported from the `den:process` module.

pub mod env;
pub mod lookup;
#[path = "signal.rs"] pub mod process_signal;
#[path = "spawn.rs"] pub mod process_spawn;
use either::Either;
pub use process_signal as signal;
pub use process_spawn as spawn;
use rquickjs::{
    Ctx, Exception, Function, Object, Result, Value, class::Class, function::Opt, object::Accessor,
};

use crate::{
    env::Env,
    signal::{Signal, SignalHub},
    spawn::Child,
};

pub fn pid() -> u32 { std::process::id() }

pub fn ppid() -> u32 { parent_id() }

pub fn argv() -> Vec<String> { std::env::args().collect() }

fn parent_id() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: getppid is a pure query of the calling process.
        unsafe { libc::getppid() as u32 }
    }
    #[cfg(windows)]
    {
        windows_parent_id()
    }
    #[cfg(not(any(unix, windows)))]
    {
        0
    }
}

#[cfg(windows)]
fn windows_parent_id() -> u32 {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Threading::GetCurrentProcessId,
        },
    };

    // SAFETY: ToolHelp snapshot iteration is the documented way to read
    // `th32ParentProcessID`; the snapshot handle is closed on every path.
    unsafe {
        let pid = GetCurrentProcessId();
        let snapshot: HANDLE = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return 0;
        }
        let mut entry = std::mem::zeroed::<PROCESSENTRY32W>();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ppid = 0;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32ProcessID == pid {
                    ppid = entry.th32ParentProcessID;
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        ppid
    }
}

#[rquickjs::function]
pub fn cwd(ctx: Ctx<'_>) -> Result<String> {
    std::env::current_dir()
        .map_err(|error| Exception::throw_internal(&ctx, &error.to_string()))?
        .into_os_string()
        .into_string()
        .map_err(|_path| Exception::throw_internal(&ctx, "cwd is not valid UTF-8"))
}

#[rquickjs::function]
pub fn chdir(dir: String, ctx: Ctx<'_>) -> Result<()> {
    std::env::set_current_dir(&dir)
        .map_err(|error| Exception::throw_internal(&ctx, &error.to_string()))
}

#[rquickjs::function]
pub fn exit(Opt(code): Opt<i32>) { std::process::exit(code.unwrap_or(0)) }

#[rquickjs::function]
pub fn spawn<'js>(
    cmd: Either<String, Vec<String>>, Opt(options): Opt<Object<'js>>, ctx: Ctx<'js>,
) -> Result<Class<'js, Child>> {
    Child::spawn(ctx, cmd, options)
}

#[rquickjs::function]
pub fn kill(pid: i32, Opt(sig): Opt<String>, ctx: Ctx<'_>) -> Result<()> {
    Signal::send(pid, sig.as_deref(), &ctx)
}

#[rquickjs::function(rename = "addSignalListener")]
pub fn add_signal_listener<'js>(sig: String, listener: Function<'js>, ctx: Ctx<'js>) -> Result<()> {
    SignalHub::add(&ctx, sig, listener)
}

#[rquickjs::function(rename = "removeSignalListener")]
pub fn remove_signal_listener<'js>(
    sig: String, listener: Function<'js>, ctx: Ctx<'js>,
) -> Result<()> {
    SignalHub::remove(&ctx, sig, listener)
}

#[rquickjs::function]
pub async fn lookup<'js>(
    host: String, Opt(options): Opt<Object<'js>>, ctx: Ctx<'js>,
) -> Result<Value<'js>> {
    lookup::host(ctx, host, options).await
}

fn install<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> Result<()> {
    SignalHub::install(ctx)?;

    let process = Object::new(ctx.clone())?;
    process.set("env", Env::proxy(ctx.clone())?)?;
    process.prop("argv", Accessor::from(argv).enumerable())?;
    process.prop("pid", Accessor::from(pid).enumerable())?;
    process.prop("ppid", Accessor::from(ppid).enumerable())?;
    process.set("cwd", js_cwd)?;
    process.set("chdir", js_chdir)?;
    process.set("exit", js_exit)?;
    process.set("spawn", js_spawn)?;
    process.set("kill", js_kill)?;
    process.set("addSignalListener", js_add_signal_listener)?;
    process.set("removeSignalListener", js_remove_signal_listener)?;
    process.set("lookup", js_lookup)?;

    exports.export("env", process.get::<_, Value>("env")?)?;
    exports.export("argv", argv())?;
    exports.export("pid", pid())?;
    exports.export("ppid", ppid())?;
    exports.export("cwd", js_cwd)?;
    exports.export("chdir", js_chdir)?;
    exports.export("exit", js_exit)?;
    exports.export("spawn", js_spawn)?;
    exports.export("kill", js_kill)?;
    exports.export("addSignalListener", js_add_signal_listener)?;
    exports.export("removeSignalListener", js_remove_signal_listener)?;
    exports.export("lookup", js_lookup)?;
    exports.export("process", process.clone())?;
    ctx.globals().set("process", process)?;
    Ok(())
}

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "PascalCase"
)]
pub mod process {
    use rquickjs::{
        Ctx, Result,
        module::{Declarations, Exports},
    };

    #[qjs(declare)]
    pub fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("env")?;
        declare.declare("argv")?;
        declare.declare("cwd")?;
        declare.declare("chdir")?;
        declare.declare("exit")?;
        declare.declare("pid")?;
        declare.declare("ppid")?;
        declare.declare("spawn")?;
        declare.declare("kill")?;
        declare.declare("addSignalListener")?;
        declare.declare("removeSignalListener")?;
        declare.declare("lookup")?;
        declare.declare("process")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        crate::install(ctx, exports)
    }
}
