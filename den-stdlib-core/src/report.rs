//! Reporting an error nobody caught — DOM §2.11 "report an exception" — and
//! the per-realm sink it goes to.

use rquickjs::{Coerced, Ctx, Error, FromJs, Function, JsLifetime, Object, Persistent, Value};

/// The property [`ExceptionSink`] looks the reporter up under, read afresh on
/// every report: a realm may *replace* its reporter long after installing the
/// bag (a worker scope does exactly that once its error chain exists), and a
/// snapshot taken at install time would miss it.
const REPORTER: &str = "reportException";

/// The object whose [`REPORTER`] property is this realm's "report an exception"
/// sink.
///
/// Without one, reporting prints. With one — `den:worker` installs its natives
/// bag here — every reporter in the process funnels through the same JS
/// function, which is what lets a worker turn *any* uncaught exception, whoever
/// caught it last, into an `error` event on its global and then on its parent.
struct ExceptionSink(Persistent<Object<'static>>);

// SAFETY: `ExceptionSink` borrows no `'js` lifetime — a `Persistent` owns its
// value outright and is tied to the runtime, not to a scope — so the type is
// the same type for every choice of `'to`.
unsafe impl<'js> JsLifetime<'js> for ExceptionSink {
    type Changed<'to> = ExceptionSink;
}

/// Point this realm's reporting at `sink[REPORTER]`.
///
/// Idempotent by construction: storing replaces whatever was there, and every
/// context gets at most one bag.
pub fn set_exception_sink<'js>(ctx: &Ctx<'js>, sink: &Object<'js>) -> rquickjs::Result<()> {
    ctx.store_userdata(ExceptionSink(Persistent::save(ctx, sink.clone())))
        .map(|_| ())
        .map_err(|_| rquickjs::Error::UserData(rquickjs::runtime::UserDataError(())))
}

/// Report `value` — a caught exception, or anything else that was thrown —
/// through this realm's sink, falling back to [`print_exception`].
///
/// Host → JS calls made from a spawned task (a timer callback, a worker's
/// `onmessage`) have no caller left to propagate to, so this is deliberately
/// infallible: reporting must never itself throw.
pub fn report_exception<'js>(ctx: &Ctx<'js>, value: &Value<'js>) {
    let Some(reporter) = reporter(ctx) else {
        return print_exception(ctx, value);
    };
    // A sink that throws is a broken sink, not a second exception to report:
    // routing its failure back through itself is how a reporting loop starts.
    if let Err(error) = reporter.call::<_, ()>((value.clone(),)) {
        match error {
            Error::Exception => print_exception(ctx, &ctx.catch()),
            error => eprintln!("{error}"),
        }
        print_exception(ctx, value);
    }
}

/// Print `value` on stderr, in the same shape `den`'s entry point uses for the
/// main script (`src/main.rs`): the exception's own formatting when it is one,
/// its string coercion when it is not, and a last-resort constant when even
/// that fails.
///
/// This is the end of the line — what a realm with no sink does, and what the
/// sink itself falls back to.
pub fn print_exception<'js>(ctx: &Ctx<'js>, value: &Value<'js>) {
    if let Some(exception) = value.as_exception() {
        eprintln!("{exception}")
    } else if let Ok(Coerced(text)) = Coerced::<String>::from_js(ctx, value.clone()) {
        eprintln!("{text}")
    } else {
        eprintln!("unknown error")
    }
}

/// This realm's reporter, if it has a sink carrying a callable one.
fn reporter<'js>(ctx: &Ctx<'js>) -> Option<Function<'js>> {
    sink_hook(ctx, REPORTER)
}

/// The sink's `name` entry, if this realm has a sink carrying a callable one.
/// The userdata guard is released before the caller runs any JS, so that the
/// hook is free to install a sink of its own.
///
/// The sink is `den:worker`'s private natives bag, which makes this the one way
/// a host-side caller reaches the hooks the preludes publish for each other and
/// deliberately keep off `globalThis` — `dispatchTrusted`, the marker that
/// separates an event the runtime fired from one a script dispatched, above
/// all.
pub fn sink_hook<'js>(ctx: &Ctx<'js>, name: &str) -> Option<Function<'js>> {
    let sink = ctx
        .userdata::<ExceptionSink>()?
        .0
        .clone()
        .restore(ctx)
        .ok()?;
    sink.get::<_, Function<'js>>(name).ok()
}
