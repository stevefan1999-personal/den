//! Signal names, `kill(2)`, and the JS listeners a realm's event loop delivers
//! to.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    future::Future,
    pin::pin,
};

use rquickjs::{
    AsyncContext, AsyncRuntime, Ctx, Exception, Function, JsLifetime, Persistent, Result,
    runtime::UserDataError,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

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

/// The realm's JS signal listeners and the mailbox that feeds them.
///
/// Delivery is a mailbox and not a subscription: one `tokio::spawn`ed forwarder
/// per watched signal pushes the name into `inbox`, and the realm's event loop
/// ([`Self::drive`], [`Self::deliver_while`]) is what takes it out and calls
/// into JS. That indirection is the whole point — a `ctx.spawn`ed pump would be
/// a future the runtime waits for, so a script whose only business is listening
/// for a signal would never go idle and `den script.js` would never exit.
pub struct SignalHub {
    listeners: RefCell<HashMap<String, Vec<Persistent<Function<'static>>>>>,
    /// Signals with a forwarder. Entries are never removed: tokio installs its
    /// handler once per process and cannot uninstall it, so a second forwarder
    /// for the same signal would only duplicate every delivery.
    watching:  RefCell<HashSet<String>>,
    /// What [`Self::remove`] took away from tokio when it handed a signal back
    /// to the kernel, so that a later [`Self::add`] can put it back — tokio's
    /// registry is a `OnceLock` and will not install its handler twice.
    #[cfg(unix)]
    handlers:  RefCell<HashMap<String, libc::sigaction>>,
    inbox_tx:  UnboundedSender<String>,
    /// Taken by whichever phase of the event loop is running and put back when
    /// that phase ends. The entry module (under [`Self::deliver_while`]) and
    /// the loop that follows it ([`Self::drive`]) are two phases of one
    /// process: a receiver consumed by the first would leave every signal after
    /// it queued for ever.
    inbox:     RefCell<Option<UnboundedReceiver<String>>>,
}

impl Default for SignalHub {
    fn default() -> Self {
        let (inbox_tx, inbox) = unbounded_channel();
        Self {
            listeners: RefCell::default(),
            watching: RefCell::default(),
            #[cfg(unix)]
            handlers: RefCell::default(),
            inbox_tx,
            inbox: RefCell::new(Some(inbox)),
        }
    }
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
        let first = {
            let mut listeners = hub.listeners.borrow_mut();
            let list = listeners.entry(sig.clone()).or_default();
            list.push(Persistent::save(ctx, listener));
            list.len() == 1
        };
        if first && hub.watching.borrow_mut().insert(sig.clone()) {
            let inbox_tx = hub.inbox_tx.clone();
            drop(hub);
            return Self::watch(ctx, sig, inbox_tx);
        }
        // The forwarder for an already-watched signal is still running, but the
        // kernel disposition `remove` handed back is not something it can undo.
        #[cfg(unix)]
        if first && let Some(previous) = hub.handlers.borrow_mut().remove(&sig) {
            Self::set_disposition(Signal::number(&sig, ctx)?, &previous);
        }
        Ok(())
    }

    pub fn remove<'js>(ctx: &Ctx<'js>, sig: String, listener: Function<'js>) -> Result<()> {
        let Some(hub) = ctx.userdata::<Self>() else {
            return Ok(());
        };
        let emptied = {
            let mut listeners = hub.listeners.borrow_mut();
            listeners.get_mut(&sig).is_some_and(|list| {
                list.retain(|saved| {
                    saved
                        .clone()
                        .restore(ctx)
                        .map(|func| func != listener)
                        .unwrap_or(true)
                });
                list.is_empty()
            }) && listeners.remove(&sig).is_some()
        };
        if emptied {
            // Node and Bun give the signal back to the kernel here, and that is
            // what makes the next Ctrl-C fatal even inside a tight JS loop:
            // nothing of den's has to run for it. `watching` keeps its entry.
            Self::flush();
            #[cfg(unix)]
            if let Some(previous) =
                Self::set_disposition(Signal::number(&sig, ctx)?, &Self::default_disposition())
            {
                hub.handlers.borrow_mut().insert(sig, previous);
            }
        }
        Ok(())
    }

    /// Start forwarding `sig` into this realm's inbox, for as long as the
    /// process lives.
    ///
    /// A `tokio::spawn`ed task and deliberately not `ctx.spawn`: a listener
    /// must never be something the runtime is waiting for. It is also never
    /// torn down — tokio's handler cannot be uninstalled anyway, so this task
    /// is what carries a signal raised after the last listener went away, and a
    /// second forwarder for the same signal would double every delivery.
    fn watch(ctx: &Ctx<'_>, sig: String, inbox_tx: UnboundedSender<String>) -> Result<()> {
        #[cfg(unix)]
        {
            let Some(kind) = Self::kind(&sig) else {
                return Ok(());
            };
            let mut stream = tokio::signal::unix::signal(kind)
                .map_err(|error| Exception::throw_internal(ctx, &error.to_string()))?;
            tokio::spawn(async move {
                while stream.recv().await.is_some() && inbox_tx.send(sig.clone()).is_ok() {}
            });
            Ok(())
        }
        #[cfg(windows)]
        {
            let stream = if sig == "SIGINT" {
                tokio::signal::windows::ctrl_c()
            } else if sig == "SIGBREAK" {
                tokio::signal::windows::ctrl_break()
            } else {
                return Ok(());
            };
            let mut stream =
                stream.map_err(|error| Exception::throw_internal(ctx, &error.to_string()))?;
            tokio::spawn(async move {
                while stream.recv().await.is_some() && inbox_tx.send(sig.clone()).is_ok() {}
            });
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (ctx, sig, inbox_tx);
            Ok(())
        }
    }

    /// Call every listener for `sig`, over a *clone* of the list: the
    /// documented graceful-shutdown recipe has the listener remove itself, and
    /// that would panic on the live `RefCell`.
    fn deliver(ctx: &Ctx<'_>, sig: &str) {
        let listeners = ctx
            .userdata::<Self>()
            .map(|hub| hub.listeners.borrow().get(sig).cloned().unwrap_or_default())
            .unwrap_or_default();
        if listeners.is_empty() {
            return Self::default_action(ctx, sig);
        }
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

    /// What the kernel would have done. Reachable only for a signal already in
    /// flight when the last listener went away: [`Self::remove`] gives the
    /// disposition back there and then, so normally the kernel — and not this —
    /// does the killing.
    fn default_action(ctx: &Ctx<'_>, sig: &str) {
        Self::flush();
        let Ok(number) = Signal::number(sig, ctx) else {
            // A name no forwarder of ours can produce; the pending throw is
            // still not something to leave behind for the next JS call.
            let _ = ctx.catch();
            return;
        };
        #[cfg(unix)]
        {
            Self::set_disposition(number, &Self::default_disposition());
            // SAFETY: raising a signal whose disposition is now the kernel's.
            unsafe { libc::raise(number) };
        }
        #[cfg(not(unix))]
        std::process::exit(128 + number);
    }

    /// Nothing after a disposition goes back to the kernel is guaranteed to
    /// run: the next signal ends the process where it stands.
    fn flush() {
        use std::io::Write as _;

        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
    }

    #[cfg(unix)]
    fn kind(sig: &str) -> Option<tokio::signal::unix::SignalKind> {
        use tokio::signal::unix::SignalKind;

        Some(match sig {
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
            _ => return None,
        })
    }

    /// Install `action` for `number`, returning what was there before.
    #[cfg(unix)]
    fn set_disposition(number: i32, action: &libc::sigaction) -> Option<libc::sigaction> {
        let mut previous = std::mem::MaybeUninit::<libc::sigaction>::uninit();
        // SAFETY: `number` came from a known signal name, and both pointers are
        // to correctly typed local storage `sigaction` may read and fill.
        let installed = unsafe { libc::sigaction(number, action, previous.as_mut_ptr()) } == 0;
        // SAFETY: the kernel filled `previous` on success.
        installed.then(|| unsafe { previous.assume_init() })
    }

    /// `SIG_DFL`, no flags, empty mask.
    #[cfg(unix)]
    fn default_disposition() -> libc::sigaction {
        // SAFETY: `sigaction` is a plain C struct with no invalid bit patterns.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = libc::SIG_DFL;
        action
    }

    /// The realm's root event loop: poll it until nothing is left spawned,
    /// delivering signals to JS in between.
    ///
    /// `idle()` owns the runtime lock for as long as it stays pending, so the
    /// only way a listener can run is to *drop* it. That cancels nothing: the
    /// spawner keeps every future and re-polls them when the next `idle()`
    /// starts.
    pub async fn drive(runtime: &AsyncRuntime, context: &AsyncContext) {
        let mut inbox = context.with(|ctx| Self::take_inbox(&ctx)).await;
        loop {
            tokio::select! {
              biased;
              sig = Self::recv(&mut inbox) => context.with(|ctx| Self::deliver(&ctx, &sig)).await,
              () = runtime.idle() => break,
            }
        }
        context.with(|ctx| Self::put_inbox(&ctx, inbox)).await;
    }

    /// Deliver signals while `entry` runs, for the entry module's sake: a
    /// server spends its whole life inside one top-level await, and a signal
    /// that lands there is its only Ctrl-C.
    ///
    /// `async_with` gives the runtime lock up at every `Pending`, so a listener
    /// can run while the entry is parked. The entry is polled through `&mut`
    /// and never dropped.
    pub async fn deliver_while<T>(context: &AsyncContext, entry: impl Future<Output = T>) -> T {
        let mut inbox = context.with(|ctx| Self::take_inbox(&ctx)).await;
        let mut entry = pin!(entry);
        let out = loop {
            tokio::select! {
              biased;
              sig = Self::recv(&mut inbox) => context.with(|ctx| Self::deliver(&ctx, &sig)).await,
              out = &mut entry => break out,
            }
        };
        context.with(|ctx| Self::put_inbox(&ctx, inbox)).await;
        out
    }

    /// The inbox arm of a root `select!`.
    ///
    /// With no receiver — no hub, or a phase that was handed `None` — it has to
    /// sleep for ever. An arm that is ready disables itself after the first
    /// poll, and no signal would ever be delivered again.
    async fn recv(inbox: &mut Option<UnboundedReceiver<String>>) -> String {
        match inbox {
            Some(receiver) => {
                match receiver.recv().await {
                    Some(sig) => sig,
                    None => std::future::pending().await,
                }
            }
            None => std::future::pending().await,
        }
    }

    fn take_inbox(ctx: &Ctx<'_>) -> Option<UnboundedReceiver<String>> {
        ctx.userdata::<Self>()
            .and_then(|hub| hub.inbox.borrow_mut().take())
    }

    fn put_inbox(ctx: &Ctx<'_>, inbox: Option<UnboundedReceiver<String>>) {
        if let Some(hub) = ctx.userdata::<Self>() {
            *hub.inbox.borrow_mut() = inbox;
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
