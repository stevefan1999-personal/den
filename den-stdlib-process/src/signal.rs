//! Signal names, `kill(2)`, and extra JS listeners on top of the binary's
//! ctrl-c handler.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};

use rquickjs::{Ctx, Exception, Function, JsLifetime, Persistent, Result, runtime::UserDataError};

/// A POSIX-style signal name (`SIGINT`, `SIGTERM`, …).
pub struct Signal;

impl Signal {
    pub fn number(name: &str, ctx: &Ctx<'_>) -> Result<i32> {
        #[cfg(unix)]
        {
            let number = match name {
                "SIGHUP" => libc::SIGHUP,
                "SIGINT" => libc::SIGINT,
                "SIGQUIT" => libc::SIGQUIT,
                "SIGILL" => libc::SIGILL,
                "SIGTRAP" => libc::SIGTRAP,
                "SIGABRT" => libc::SIGABRT,
                "SIGBUS" => libc::SIGBUS,
                "SIGFPE" => libc::SIGFPE,
                "SIGKILL" => libc::SIGKILL,
                "SIGUSR1" => libc::SIGUSR1,
                "SIGSEGV" => libc::SIGSEGV,
                "SIGUSR2" => libc::SIGUSR2,
                "SIGPIPE" => libc::SIGPIPE,
                "SIGALRM" => libc::SIGALRM,
                "SIGTERM" => libc::SIGTERM,
                "SIGCHLD" => libc::SIGCHLD,
                "SIGCONT" => libc::SIGCONT,
                "SIGSTOP" => libc::SIGSTOP,
                "SIGTSTP" => libc::SIGTSTP,
                "SIGTTIN" => libc::SIGTTIN,
                "SIGTTOU" => libc::SIGTTOU,
                "SIGURG" => libc::SIGURG,
                "SIGXCPU" => libc::SIGXCPU,
                "SIGXFSZ" => libc::SIGXFSZ,
                "SIGVTALRM" => libc::SIGVTALRM,
                "SIGPROF" => libc::SIGPROF,
                "SIGWINCH" => libc::SIGWINCH,
                "SIGIO" => libc::SIGIO,
                #[cfg(any(target_os = "linux", target_os = "android"))]
                "SIGSTKFLT" => libc::SIGSTKFLT,
                #[cfg(any(target_os = "linux", target_os = "android"))]
                "SIGPWR" => libc::SIGPWR,
                "SIGSYS" => libc::SIGSYS,
                _ => {
                    return Err(Exception::throw_type(
                        ctx,
                        &format!("unknown signal {name}"),
                    ));
                }
            };
            Ok(number)
        }
        #[cfg(windows)]
        {
            match name {
                "SIGINT" | "SIGTERM" | "SIGKILL" | "SIGBREAK" => Ok(1),
                _ => {
                    Err(Exception::throw_type(
                        ctx,
                        &format!("unsupported signal {name}"),
                    ))
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = name;
            Err(Exception::throw_type(ctx, "signals are not supported"))
        }
    }

    pub fn send(pid: i32, name: Option<&str>, ctx: &Ctx<'_>) -> Result<()> {
        let name = name.unwrap_or("SIGTERM");
        #[cfg(unix)]
        {
            let number = Self::number(name, ctx)?;
            // SAFETY: `kill(2)` takes a pid and a signal number; both are
            // integers, and `number` came from a known signal name.
            let rc = unsafe { libc::kill(pid as libc::pid_t, number) };
            if rc != 0 {
                return Err(Exception::throw_internal(
                    ctx,
                    &std::io::Error::last_os_error().to_string(),
                ));
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            let _ = Self::number(name, ctx)?;
            Self::windows_kill(pid as u32, ctx)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = pid;
            Err(Exception::throw_internal(ctx, "kill is not supported"))
        }
    }

    pub fn can_listen(name: &str) -> bool {
        matches!(
            name,
            "SIGINT"
                | "SIGTERM"
                | "SIGHUP"
                | "SIGQUIT"
                | "SIGUSR1"
                | "SIGUSR2"
                | "SIGCHLD"
                | "SIGALRM"
                | "SIGPIPE"
                | "SIGIO"
                | "SIGWINCH"
                | "SIGBREAK"
        )
    }
}

/// Extra JS listeners. They do not replace the binary's `ctrl_c` handler:
/// tokio's signal registry fans the same delivery out to every subscriber.
#[derive(Default)]
pub struct SignalHub {
    listeners: RefCell<HashMap<String, Vec<Persistent<Function<'static>>>>>,
    watching:  RefCell<HashSet<String>>,
}

// SAFETY: the hub stores `Persistent` handles tied to the runtime, not to a
// `'js` borrow, so the type is the same for every lifetime.
unsafe impl<'js> JsLifetime<'js> for SignalHub {
    type Changed<'to> = SignalHub;
}

impl SignalHub {
    pub fn install(ctx: &Ctx<'_>) -> Result<()> {
        ctx.store_userdata(Self::default())
            .map(|_| ())
            .map_err(|_| rquickjs::Error::UserData(UserDataError(())))
    }

    pub fn add<'js>(ctx: &Ctx<'js>, sig: String, listener: Function<'js>) -> Result<()> {
        if !Signal::can_listen(&sig) {
            // SIGKILL/SIGSTOP and friends cannot be caught; match the platform.
            let _ = Signal::number(&sig, ctx)?;
            return Err(Exception::throw_type(
                ctx,
                &format!("cannot listen for {sig}"),
            ));
        }
        #[cfg(windows)]
        if !matches!(sig.as_str(), "SIGINT" | "SIGBREAK") {
            return Err(Exception::throw_type(
                ctx,
                &format!("cannot listen for {sig}"),
            ));
        }

        let Some(hub) = ctx.userdata::<Self>() else {
            return Err(Exception::throw_internal(
                ctx,
                "signal hub is not installed",
            ));
        };
        let start_watch = {
            let mut listeners = hub.listeners.borrow_mut();
            let list = listeners.entry(sig.clone()).or_default();
            list.push(Persistent::save(ctx, listener));
            list.len() == 1 && !hub.watching.borrow().contains(&sig)
        };
        if start_watch {
            hub.watching.borrow_mut().insert(sig.clone());
            drop(hub);
            Self::watch(ctx, sig)?;
        }
        Ok(())
    }

    pub fn remove<'js>(ctx: &Ctx<'js>, sig: String, listener: Function<'js>) -> Result<()> {
        let Some(hub) = ctx.userdata::<Self>() else {
            return Ok(());
        };
        let mut listeners = hub.listeners.borrow_mut();
        if let Some(list) = listeners.get_mut(&sig) {
            list.retain(|saved| {
                saved
                    .clone()
                    .restore(ctx)
                    .map(|func| func != listener)
                    .unwrap_or(true)
            });
            if list.is_empty() {
                listeners.remove(&sig);
            }
        }
        Ok(())
    }

    fn watch(ctx: &Ctx<'_>, sig: String) -> Result<()> {
        let ctx = ctx.clone();
        ctx.clone().spawn(async move {
            let _ = Self::listen_loop(&ctx, &sig).await;
        });
        Ok(())
    }

    fn dispatch(ctx: &Ctx<'_>, sig: &str) {
        let Some(hub) = ctx.userdata::<Self>() else {
            return;
        };
        let listeners = hub.listeners.borrow().get(sig).cloned().unwrap_or_default();
        drop(hub);
        for saved in listeners {
            let Ok(func) = saved.restore(ctx) else {
                continue;
            };
            match func.call::<_, ()>(()) {
                Ok(()) => {}
                Err(rquickjs::Error::Exception) => {
                    let caught = ctx.catch();
                    match caught.as_exception() {
                        Some(exception) => eprintln!("{exception}"),
                        None => eprintln!("{caught:?}"),
                    }
                }
                Err(error) => eprintln!("{error}"),
            }
        }
    }

    async fn listen_loop(ctx: &Ctx<'_>, sig: &str) -> Result<()> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};

            let kind = match sig {
                "SIGINT" => SignalKind::interrupt(),
                "SIGTERM" => SignalKind::terminate(),
                "SIGHUP" => SignalKind::hangup(),
                "SIGQUIT" => SignalKind::quit(),
                "SIGUSR1" => SignalKind::user_defined1(),
                "SIGUSR2" => SignalKind::user_defined2(),
                "SIGCHLD" => SignalKind::child(),
                "SIGALRM" => SignalKind::alarm(),
                "SIGPIPE" => SignalKind::pipe(),
                "SIGIO" => SignalKind::io(),
                "SIGWINCH" => SignalKind::window_change(),
                _ => return Ok(()),
            };
            let mut stream =
                signal(kind).map_err(|error| Exception::throw_internal(ctx, &error.to_string()))?;
            while stream.recv().await.is_some() {
                Self::dispatch(ctx, sig);
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            match sig {
                "SIGINT" => {
                    let mut stream = tokio::signal::windows::ctrl_c()
                        .map_err(|error| Exception::throw_internal(ctx, &error.to_string()))?;
                    while stream.recv().await.is_some() {
                        Self::dispatch(ctx, sig);
                    }
                }
                "SIGBREAK" => {
                    let mut stream = tokio::signal::windows::ctrl_break()
                        .map_err(|error| Exception::throw_internal(ctx, &error.to_string()))?;
                    while stream.recv().await.is_some() {
                        Self::dispatch(ctx, sig);
                    }
                }
                _ => {}
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (ctx, sig);
            Ok(())
        }
    }
}

#[cfg(windows)]
impl Signal {
    fn windows_kill(pid: u32, ctx: &Ctx<'_>) -> Result<()> {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, HANDLE},
            System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess},
        };

        // SAFETY: `OpenProcess` / `TerminateProcess` / `CloseHandle` are the
        // documented Win32 sequence for killing by pid; a failed open or
        // terminate is reported, and the handle is closed on both paths.
        unsafe {
            let handle: HANDLE = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() || handle == (-1isize as HANDLE) {
                return Err(Exception::throw_internal(
                    ctx,
                    &std::io::Error::last_os_error().to_string(),
                ));
            }
            let ok = TerminateProcess(handle, 1);
            CloseHandle(handle);
            if ok == 0 {
                return Err(Exception::throw_internal(
                    ctx,
                    &std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        Ok(())
    }
}
