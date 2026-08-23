//! Process, environment, spawn, signals and DNS lookup (`den:process`).
//!
//! The same object is installed as `globalThis.process` (via `evaluate_def`)
//! and exported from the `den:process` module.

pub mod env;
pub mod lookup;
pub mod signal;
pub mod spawn;

use either::Either;
use rquickjs::{
    Ctx, Exception, Function, Object, Result, Value, class::Class, function::Opt, object::Accessor,
};

use crate::{
    env::Env,
    lookup::Lookup,
    signal::{Signal, SignalHub},
    spawn::Child,
};

/// Host-side process helpers. JS sees the object `Process::install` builds.
pub struct Process;

impl Process {
    pub fn pid() -> u32 {
        std::process::id()
    }

    pub fn ppid() -> u32 {
        ParentId::get()
    }

    pub fn argv() -> Vec<String> {
        std::env::args().collect()
    }

    pub fn cwd(ctx: &Ctx<'_>) -> Result<String> {
        std::env::current_dir()
            .map_err(|error| Exception::throw_internal(ctx, &error.to_string()))?
            .into_os_string()
            .into_string()
            .map_err(|_| Exception::throw_internal(ctx, "cwd is not valid UTF-8"))
    }

    pub fn chdir(dir: String, ctx: &Ctx<'_>) -> Result<()> {
        std::env::set_current_dir(&dir)
            .map_err(|error| Exception::throw_internal(ctx, &error.to_string()))
    }

    pub fn exit(code: Option<i32>) -> ! {
        std::process::exit(code.unwrap_or(0))
    }

    pub fn install<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> Result<()> {
        SignalHub::install(ctx)?;

        let process = Object::new(ctx.clone())?;
        process.set("env", Env::proxy(ctx.clone())?)?;
        process.prop("argv", Accessor::from(Self::argv).enumerable())?;
        process.prop("pid", Accessor::from(Self::pid).enumerable())?;
        process.prop("ppid", Accessor::from(Self::ppid).enumerable())?;

        process.set("cwd", js_cwd)?;
        process.set("chdir", js_chdir)?;
        process.set("exit", js_exit)?;
        process.set("spawn", js_spawn)?;
        process.set("kill", js_kill)?;
        process.set("addSignalListener", js_add_signal_listener)?;
        process.set("removeSignalListener", js_remove_signal_listener)?;
        process.set("lookup", js_lookup)?;

        exports.export("env", process.get::<_, Value>("env")?)?;
        exports.export("argv", Self::argv())?;
        exports.export("pid", Self::pid())?;
        exports.export("ppid", Self::ppid())?;
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
}

struct ParentId;

impl ParentId {
    fn get() -> u32 {
        #[cfg(unix)]
        {
            // SAFETY: getppid is a pure query of the calling process.
            unsafe { libc::getppid() as u32 }
        }
        #[cfg(windows)]
        {
            Self::windows()
        }
        #[cfg(not(any(unix, windows)))]
        {
            0
        }
    }
}

#[cfg(windows)]
impl ParentId {
    fn windows() -> u32 {
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
}

#[rquickjs::function]
pub fn cwd(ctx: Ctx<'_>) -> Result<String> {
    Process::cwd(&ctx)
}

#[rquickjs::function]
pub fn chdir(dir: String, ctx: Ctx<'_>) -> Result<()> {
    Process::chdir(dir, &ctx)
}

#[rquickjs::function]
pub fn exit(Opt(code): Opt<i32>) {
    Process::exit(code)
}

#[rquickjs::function]
pub fn spawn<'js>(
    cmd: Either<String, Vec<String>>,
    Opt(options): Opt<Object<'js>>,
    ctx: Ctx<'js>,
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
    sig: String,
    listener: Function<'js>,
    ctx: Ctx<'js>,
) -> Result<()> {
    SignalHub::remove(&ctx, sig, listener)
}

#[rquickjs::function]
pub async fn lookup<'js>(
    host: String,
    Opt(options): Opt<Object<'js>>,
    ctx: Ctx<'js>,
) -> Result<Value<'js>> {
    Lookup::host(ctx, host, options).await
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
        crate::Process::install(ctx, exports)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rquickjs::{
        AsyncContext, AsyncRuntime, CatchResultExt, FromJs, Module, Object, Promise,
        context::EvalOptions,
    };

    /// `chdir` mutates process-global state; tests that touch it take this
    /// lock.
    static CWD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Evaluate `source` in a fresh realm with `den:process` installed.
    ///
    /// One runtime per call: the module parks a signal hub in context userdata.
    /// The snippet may use top-level `await`.
    async fn eval<T>(source: &str) -> Result<T, String>
    where
        T: for<'js> FromJs<'js> + Send + Sync + 'static,
    {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        context
            .async_with(async |ctx| {
                let run = async {
                    let (_module, evaluated) =
                        Module::evaluate_def::<crate::js_process, _>(ctx.clone(), "den:process")?;
                    evaluated.into_future::<()>().await?;
                    let mut options = EvalOptions::default();
                    options.global = true;
                    options.promise = true;
                    options.strict = true;
                    ctx.eval_with_options::<Promise, _>(source, options)?
                        .into_future::<Object>()
                        .await?
                        .get::<_, T>("value")
                };
                run.await.catch(&ctx).map_err(|err| err.to_string())
            })
            .await
    }

    fn echo_argv() -> Vec<String> {
        #[cfg(windows)]
        {
            vec![
                "cmd".into(),
                "/c".into(),
                "echo".into(),
                "hello-from-den".into(),
            ]
        }
        #[cfg(not(windows))]
        {
            let program = ["/bin/echo", "/usr/bin/echo"]
                .into_iter()
                .find(|path| PathBuf::from(path).exists())
                .unwrap_or("echo");
            vec![program.into(), "hello-from-den".into()]
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn argv_is_a_non_empty_array_of_strings() {
        let failures: String = eval(
            r#"
              Object.entries({
                isArray: Array.isArray(process.argv),
                nonEmpty: process.argv.length > 0,
                allStrings: process.argv.every((arg) => typeof arg === "string"),
              }).filter(([, held]) => !held).map(([name]) => name).join(",")
            "#,
        )
        .await
        .expect("argv evaluates");
        assert_eq!(failures, "");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn env_path_or_home_is_a_string() {
        let kind: String = eval(
            r#"
              const value = process.env.PATH ?? process.env.HOME;
              typeof value
            "#,
        )
        .await
        .expect("env evaluates");
        assert_eq!(kind, "string");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn env_get_set_delete_round_trips() {
        let key = format!("DEN_PROCESS_TEST_{}", std::process::id());
        let report: String = eval(&format!(
            r#"
              const key = {key:?};
              process.env[key] = 123;
              const afterSet = process.env[key];
              const has = key in process.env;
              delete process.env[key];
              const afterDelete = process.env[key];
              [afterSet, has, afterDelete === undefined].join(",")
            "#
        ))
        .await
        .expect("env mutation evaluates");
        assert_eq!(report, "123,true,true");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cwd_round_trips_with_chdir_in_a_temp_dir() {
        let _guard = CWD_LOCK.lock().await;
        let original = std::env::current_dir().expect("cwd");
        let dir = std::env::temp_dir().join(format!("den-process-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let dir = dir.canonicalize().expect("canonical temp dir");
        let dir_js = dir.to_string_lossy().replace('\\', "/");

        let outcome = eval::<String>(&format!(
            r#"
              const original = process.cwd();
              process.chdir({dir_js:?});
              const now = process.cwd();
              process.chdir(original);
              const restored = process.cwd();
              now + "\n" + restored
            "#
        ))
        .await;

        let _ = std::env::set_current_dir(&original);
        let payload = outcome.expect("chdir evaluates");
        let (now, restored) = payload.split_once('\n').unwrap_or((payload.as_str(), ""));
        let now_path = PathBuf::from(now)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(now));
        assert_eq!(now_path, dir, "chdir should land in the temp dir");
        let restored_path = PathBuf::from(restored)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(restored));
        assert_eq!(
            restored_path,
            original.canonicalize().unwrap_or(original),
            "chdir should restore the original cwd"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lookup_localhost_returns_loopback() {
        let ok: bool = eval(
            r#"
              const addr = await process.lookup("localhost");
              addr.ip === "127.0.0.1" || addr.ip === "::1"
            "#,
        )
        .await
        .expect("lookup evaluates");
        assert!(ok, "localhost should resolve to a loopback address");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lookup_all_returns_an_array() {
        let ok: bool = eval(
            r#"
              const addrs = await process.lookup("localhost", { all: true });
              Array.isArray(addrs) && addrs.length > 0 && addrs.every((a) => a.ip && (a.family === 4 || a.family === 6))
            "#,
        )
        .await
        .expect("lookup all evaluates");
        assert!(ok);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn spawn_echo_exits_zero_and_reads_stdout() {
        let argv = echo_argv();
        let argv_js = format!(
            "[{}]",
            argv.iter()
                .map(|arg| format!("{arg:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let report: String = eval(&format!(
            r#"
              const child = process.spawn({argv_js}, {{ stdout: "pipe", stderr: "ignore" }});
              const out = await child.stdout.text();
              const status = await child.wait();
              [child.pid > 0, status.code, out.trim()].join("|")
            "#
        ))
        .await
        .expect("spawn evaluates");
        let parts: Vec<&str> = report.split('|').collect();
        assert_eq!(
            parts.first().copied(),
            Some("true"),
            "pid should be positive: {report}"
        );
        assert_eq!(
            parts.get(1).copied(),
            Some("0"),
            "echo should exit 0: {report}"
        );
        assert_eq!(
            parts.get(2).copied(),
            Some("hello-from-den"),
            "stdout should contain the echo text: {report}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pid_is_a_positive_number() {
        let report: String = eval(
            r#"
              Object.entries({
                pidPositive: typeof process.pid === "number" && process.pid > 0,
                ppidPositive: typeof process.ppid === "number" && process.ppid > 0,
                exitIsFunction: typeof process.exit === "function",
              }).filter(([, held]) => !held).map(([name]) => name).join(",")
            "#,
        )
        .await
        .expect("pid evaluates");
        assert_eq!(report, "");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn add_and_remove_signal_listener_do_not_throw() {
        let ok: bool = eval(
            r#"
              const listener = () => {};
              process.addSignalListener("SIGTERM", listener);
              process.removeSignalListener("SIGTERM", listener);
              true
            "#,
        )
        .await
        .expect("signal listener registration evaluates");
        assert!(ok);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn kill_terminates_a_spawned_sleep() {
        let sleep = ["/bin/sleep", "/usr/bin/sleep"]
            .into_iter()
            .find(|path| PathBuf::from(path).exists())
            .unwrap_or("sleep");
        let report: String = eval(&format!(
            r#"
              const child = process.spawn([{sleep:?}, "30"], {{ stdout: "ignore", stderr: "ignore" }});
              process.kill(child.pid, "SIGKILL");
              const status = await child.wait();
              [child.pid > 0, status.code === null].join("|")
            "#
        ))
        .await
        .expect("kill evaluates");
        assert_eq!(report, "true|true", "SIGKILL should leave a null exit code");
    }
}
