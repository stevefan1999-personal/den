//! Natives backing `src/prelude/events.js`.
//!
//! `EventTarget`, `Event`, `MessageEvent` and `ErrorEvent` are pure JS — they
//! need nothing from the host except one thing JS cannot do: DOM §2.11 "report
//! an exception", the sink for an exception thrown by a listener, which has to
//! reach the process' stderr the same way an uncaught error from the main
//! script does.

use den_stdlib_core::report::{print_exception, set_exception_sink};
use rquickjs::{Ctx, Object, Result, Value};

/// `natives.reportException(value)` — DOM §2.11. A listener that throws does
/// not stop the dispatch and has no `catch` to fall into, so the exception is
/// reported and discarded.
///
/// This is the *printer*, deliberately not the dispatcher: it is what the
/// realm's sink falls back to (and what `worker.js` keeps hold of before
/// replacing the sink entry), so dispatching from here would call it right
/// back.
#[rquickjs::function(rename = "reportException")]
pub fn report_exception<'js>(ctx: Ctx<'js>, value: Value<'js>) {
    print_exception(&ctx, &value)
}

/// Add this module's natives to the `natives` bag the prelude is called with.
pub fn install<'js>(ctx: &Ctx<'js>, natives: &Object<'js>) -> Result<()> {
    natives.set("reportException", js_report_exception)?;
    // Every reporter in the realm — a listener that throws (events.js), a timer
    // callback (den-stdlib-timer), a port pump — now resolves this one bag
    // entry at report time. That is what makes `worker.js` replacing it reach
    // the *Rust* reporters too, and so what puts a throwing `setTimeout` body
    // on the worker's error chain instead of straight onto stderr.
    set_exception_sink(ctx, natives)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rquickjs::{
        CatchResultExt, Context, FromJs, Function, Object, Result, Runtime, Value,
        context::EvalOptions,
    };

    /// The prelude under test. It is deliberately evaluated on its own rather
    /// than through `den:worker`: these are unit tests of the event layer, and
    /// pulling in the whole module would couple them to every other prelude.
    const EVENTS_PRELUDE: &str = include_str!("prelude/events.js");

    /// Names the tests reach for. `__defineEventHandler` is internal plumbing
    /// that `den:worker` does not export, so a test is the only other caller.
    const EXPOSED: [&str; 7] = [
        "CustomEvent",
        "ErrorEvent",
        "Event",
        "EventTarget",
        "MessageEvent",
        "reportError",
        "__defineEventHandler",
    ];

    /// A fresh runtime whose globals are exactly what `events.js` returns.
    fn events_context() -> (Runtime, Context) {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let install = || -> Result<()> {
                let natives = Object::new(ctx.clone())?;
                super::install(&ctx, &natives)?;
                let mut options = EvalOptions::default();
                options.filename = Some("den:worker/events.js".to_owned());
                let factory: Function<'_> = ctx.eval_with_options(EVENTS_PRELUDE, options)?;
                let api: Object<'_> = factory.call((natives.clone(), Object::new(ctx.clone())?))?;
                let globals = ctx.globals();
                for name in EXPOSED {
                    globals.set(name, api.get::<_, Value<'_>>(name)?)?;
                }
                // The private bag, which in production only the later preludes
                // hold: `dispatchTrusted` lives there precisely so that no
                // script can reach it, and `reportException` is replaceable
                // there, which is how a worker realm redirects it. Both are
                // behaviour a test has to be able to see.
                globals.set("__natives", natives)?;
                Ok(())
            };
            install()
                .catch(&ctx)
                .map_err(|err| err.to_string())
                .expect("events prelude evaluates");
        });
        (runtime, context)
    }

    /// Evaluate `source` as a script; its last expression is the result.
    fn eval<T>(source: &str) -> std::result::Result<T, String>
    where
        T: for<'js> FromJs<'js>,
    {
        let (_runtime, context) = events_context();
        context.with(|ctx| {
            ctx.eval::<T, _>(source)
                .catch(&ctx)
                .map_err(|err| err.to_string())
        })
    }

    /// Same, for the many tests whose script builds a comma-joined trace.
    fn trace(source: &str) -> String {
        eval::<String>(source).expect("script evaluates")
    }

    #[test]
    fn listeners_run_in_registration_order_and_once_removes_itself() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                const first = () => log.push("first");
                target.addEventListener("ping", first);
                target.addEventListener("ping", () => log.push("once"), { once: true });
                // A second add with the same (type, callback, capture) is dropped.
                target.addEventListener("ping", first);
                target.addEventListener("ping", () => log.push("last"));
                target.dispatchEvent(new Event("ping"));
                target.removeEventListener("ping", first);
                target.dispatchEvent(new Event("ping"));
                log.join(",")
                "#
            ),
            "first,once,last,last"
        );
    }

    #[test]
    fn a_listener_removed_during_dispatch_does_not_run() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                const second = () => log.push("second");
                target.addEventListener("ping", () => {
                  log.push("first");
                  target.removeEventListener("ping", second);
                });
                target.addEventListener("ping", second);
                target.addEventListener("ping", () => log.push("third"));
                target.dispatchEvent(new Event("ping"));
                log.join(",")
                "#
            ),
            "first,third"
        );
    }

    #[test]
    fn a_listener_added_during_dispatch_runs_only_on_the_next_one() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                target.addEventListener("ping", () => {
                  log.push("first");
                  target.addEventListener("ping", () => log.push("added"));
                }, { once: true });
                target.dispatchEvent(new Event("ping"));
                target.dispatchEvent(new Event("ping"));
                log.join(",")
                "#
            ),
            "first,added"
        );
    }

    #[test]
    fn stop_immediate_propagation_skips_the_remaining_listeners() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                target.addEventListener("ping", (event) => {
                  log.push("first");
                  event.stopImmediatePropagation();
                }, { once: true });
                target.addEventListener("ping", () => log.push("second"));
                const returned = target.dispatchEvent(new Event("ping"));
                // The flag is cleared at the end of dispatch, so the next one is whole.
                target.dispatchEvent(new Event("ping"));
                `${log.join(",")}|${returned}`
                "#
            ),
            "first,second|true"
        );
    }

    #[test]
    fn prevent_default_only_cancels_a_cancelable_event() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                let prevented = "unset";
                target.addEventListener("ping", (event) => {
                  event.preventDefault();
                  prevented = String(event.defaultPrevented);
                });
                const cancelable = target.dispatchEvent(new Event("ping", { cancelable: true }));
                const plain = target.dispatchEvent(new Event("ping"));
                `${cancelable},${plain},${prevented}`
                "#
            ),
            "false,true,false"
        );
    }

    #[test]
    fn a_throwing_listener_is_reported_and_the_next_one_still_runs() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                target.addEventListener("ping", () => {
                  log.push("threw");
                  throw new Error("reported to stderr, not to the caller");
                });
                target.addEventListener("ping", () => log.push("after"));
                const returned = target.dispatchEvent(new Event("ping"));
                `${log.join(",")}|${returned}`
                "#
            ),
            "threw,after|true"
        );
    }

    #[test]
    fn a_handle_event_object_is_a_listener_and_null_is_tolerated() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                target.addEventListener("ping", null);
                target.addEventListener("ping", {
                  handleEvent(event) { log.push(`${event.type}:${this.tag}`) },
                  tag: "object",
                });
                // A listener object without handleEvent throws a TypeError, which is
                // reported like any other listener exception.
                target.addEventListener("ping", {});
                target.addEventListener("ping", () => log.push("function"));
                target.dispatchEvent(new Event("ping"));
                log.join(",")
                "#
            ),
            "ping:object,function"
        );
    }

    #[test]
    fn dispatch_sets_target_and_phase_and_clears_them_afterwards() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                let inside = "unset";
                target.addEventListener("ping", function (event) {
                  inside = [
                    event.target === target, event.currentTarget === target,
                    event.eventPhase === Event.AT_TARGET, this === target, event.isTrusted,
                  ].join(",");
                });
                const event = new Event("ping");
                target.dispatchEvent(event);
                `${inside}|${event.target === target},${event.currentTarget},${event.eventPhase}`
                "#
            ),
            "true,true,true,true,false|true,null,0"
        );
    }

    #[test]
    fn re_dispatching_a_dispatching_event_throws_invalid_state_error() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                let name = "nothing thrown";
                target.addEventListener("ping", (event) => {
                  try { target.dispatchEvent(event) } catch (error) { name = error.name }
                });
                const event = new Event("ping");
                target.dispatchEvent(event);
                // The dispatch flag is cleared afterwards: the same event redispatches.
                `${name},${target.dispatchEvent(event)}`
                "#
            ),
            "InvalidStateError,true"
        );
    }

    #[test]
    fn an_on_x_handler_keeps_the_position_of_its_first_assignment() {
        assert_eq!(
            trace(
                r#"
                class Widget extends EventTarget {}
                __defineEventHandler(Widget.prototype, "onping");
                const widget = new Widget();
                const log = [];
                widget.onping = () => log.push("one");
                widget.addEventListener("ping", () => log.push("two"));
                // Reassignment swaps the value in place, so "three" still runs first.
                widget.onping = () => log.push("three");
                widget.dispatchEvent(new Event("ping"));
                // Deactivated, then reactivated at the end of the list.
                widget.onping = null;
                widget.dispatchEvent(new Event("ping"));
                widget.onping = () => log.push("five");
                widget.dispatchEvent(new Event("ping"));
                log.join(",")
                "#
            ),
            "three,two,two,two,five"
        );
    }

    #[test]
    fn an_on_x_slot_reads_back_what_it_stores() {
        assert_eq!(
            trace(
                r#"
                class Widget extends EventTarget {}
                __defineEventHandler(Widget.prototype, "onping");
                const widget = new Widget();
                const handler = () => {};
                const initial = widget.onping;
                widget.onping = handler;
                const stored = widget.onping === handler;
                // [LegacyTreatNonObjectAsNull]: a string is stored as null.
                widget.onping = "not a callback";
                const primitive = widget.onping;
                widget.onping = handler;
                widget.onping = null;
                `${initial},${stored},${primitive},${widget.onping}`
                "#
            ),
            "null,true,null,null"
        );
    }

    #[test]
    fn a_handler_returning_false_cancels_and_a_global_onerror_takes_five_arguments() {
        assert_eq!(
            trace(
                r#"
                const plain = new EventTarget();
                __defineEventHandler(plain, "onping");
                plain.onping = () => false;
                const cancelledByFalse = plain.dispatchEvent(new Event("ping", { cancelable: true }));

                const global = new EventTarget();
                __defineEventHandler(global, "onerror", true);
                let seen = "unset";
                global.onerror = (message, filename, lineno, colno, error) => {
                  seen = [message, filename, lineno, colno, error].join(",");
                  return true;
                };
                const errorEvent = new ErrorEvent("error", {
                  cancelable: true, message: "boom", filename: "worker.js",
                  lineno: 3, colno: 7, error: "carried",
                });
                const cancelledByTrue = global.dispatchEvent(errorEvent);
                `${cancelledByFalse}|${seen}|${cancelledByTrue}`
                "#
            ),
            "false|boom,worker.js,3,7,carried|false"
        );
    }

    #[test]
    fn message_event_carries_a_frozen_port_array() {
        assert_eq!(
            trace(
                r#"
                const ports = ["first", "second"];
                const event = new MessageEvent("message", { data: 42, ports });
                ports.push("added after construction");
                const empty = new MessageEvent("message");
                [
                  event.data, Object.isFrozen(event.ports), event.ports.join("+"),
                  event.origin === "", event.lastEventId === "", event.source === null,
                  empty.data === null, Object.isFrozen(empty.ports), empty.ports.length,
                ].join(",")
                "#
            ),
            "42,true,first+second,true,true,true,true,true,0"
        );
    }

    #[test]
    fn error_event_carries_its_fields_and_defaults() {
        assert_eq!(
            trace(
                r#"
                const event = new ErrorEvent("error", {
                  message: "boom", filename: "worker.js", lineno: 3, colno: 7, error: "carried",
                });
                const empty = new ErrorEvent("error");
                [
                  event.message, event.filename, event.lineno, event.colno, event.error,
                  empty.message === "", empty.lineno, empty.colno, empty.error === undefined,
                ].join(",")
                "#
            ),
            "boom,worker.js,3,7,carried,true,0,0,true"
        );
    }

    #[test]
    fn the_classes_look_like_platform_objects() {
        assert_eq!(
            trace(
                r#"
                [
                  Object.prototype.toString.call(new EventTarget()),
                  Object.prototype.toString.call(new MessageEvent("message")),
                  Object.prototype.toString.call(new ErrorEvent("error")),
                  Object.keys(EventTarget.prototype).length,
                  EventTarget.prototype.addEventListener.length,
                  EventTarget.prototype.dispatchEvent.length,
                  Event.length, MessageEvent.name,
                  Event.AT_TARGET, new Event("ping").eventPhase,
                  new MessageEvent("message") instanceof Event,
                ].join(",")
                "#
            ),
            "[object EventTarget],[object MessageEvent],[object \
             ErrorEvent],0,2,1,1,MessageEvent,2,0,true"
        );
    }

    /// The whole point of the sink: a report made from *Rust* — a timer
    /// callback, a port pump, a worker fault — comes out of whatever reporter
    /// the realm last installed on the natives bag, rather than going straight
    /// to stderr. Replacing that entry is how `worker.js` puts an uncaught
    /// exception on the worker's error chain, and before the bag became the
    /// realm's sink only the JS-side reporters ever saw the replacement.
    #[test]
    fn a_rust_side_report_goes_through_the_reporter_the_realm_installed() {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        let seen = context.with(|ctx| {
            let report = || -> Result<String> {
                let natives = Object::new(ctx.clone())?;
                super::install(&ctx, &natives)?;
                ctx.globals().set("natives", natives)?;
                ctx.eval::<(), _>(
                    r#"globalThis.seen = "nothing";
                       natives.reportException = (value) => {
                         globalThis.seen = `chain:${value.message}`;
                       };"#,
                )?;
                let thrown = ctx.eval::<Value<'_>, _>("new Error('from rust')")?;
                den_stdlib_core::report::report_exception(&ctx, &thrown);
                ctx.globals().get::<_, String>("seen")
            };
            report()
                .catch(&ctx)
                .map_err(|err| err.to_string())
                .expect("the report is routed")
        });
        assert_eq!(seen, "chain:from rust");
    }

    #[test]
    fn dispatching_a_non_event_throws_a_type_error() {
        assert_eq!(
            trace(
                r#"
                try {
                  new EventTarget().dispatchEvent({ type: "ping" });
                  "no error"
                } catch (error) { error.name }
                "#
            ),
            "TypeError"
        );
    }

    #[test]
    fn only_the_runtime_dispatch_marks_an_event_trusted() {
        // DOM: an event fired by the user agent has `isTrusted` true, and
        // `dispatchEvent` forces false. The runtime's own `message`,
        // `messageerror` and `error` events go through the seam; everything a
        // script dispatches, including one re-dispatched from inside a trusted
        // listener, does not.
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const seen = [];
                target.addEventListener("ping", (event) => {
                  seen.push(event.isTrusted);
                  // Nested, from inside a trusted dispatch: still a script.
                  if (event.type === "ping") target.dispatchEvent(new Event("pong"));
                });
                target.addEventListener("pong", (event) => seen.push(event.isTrusted));
                __natives.dispatchTrusted(target, new Event("ping"));
                target.dispatchEvent(new Event("ping"));
                // The same event object dispatched again is no longer trusted.
                const once = new Event("ping");
                __natives.dispatchTrusted(target, once);
                target.dispatchEvent(once);
                seen.join(",")
                "#
            ),
            "true,false,false,false,true,false,false,false"
        );
    }

    #[test]
    fn a_trusted_dispatch_reports_whether_the_event_was_cancelled() {
        // The runtime's error chain branches on this return value, so the seam
        // has to pass `dispatchEvent`'s answer through unchanged.
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                target.addEventListener("boom", (event) => event.preventDefault());
                const cancelable = __natives.dispatchTrusted(target, new Event("boom", { cancelable: true }));
                const plain = __natives.dispatchTrusted(target, new Event("boom"));
                `${cancelable},${plain}`
                "#
            ),
            "false,true"
        );
    }

    #[test]
    fn composed_path_and_src_element_follow_the_dispatch() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const event = new Event("ping");
                const before = `${event.composedPath().length},${event.srcElement}`;
                let during = "";
                target.addEventListener("ping", () => {
                  const path = event.composedPath();
                  during = `${path.length},${path[0] === target},${event.srcElement === target}`;
                });
                target.dispatchEvent(event);
                `${before}|${during}|${event.composedPath().length},${event.srcElement === target}`
                "#
            ),
            "0,null|1,true,true|0,true"
        );
    }

    #[test]
    fn the_legacy_cancel_bubble_and_return_value_flags_move_one_way_only() {
        assert_eq!(
            trace(
                r#"
                const event = new Event("ping", { cancelable: true });
                const fresh = `${event.cancelBubble},${event.returnValue}`;
                event.cancelBubble = false;
                event.returnValue = true;
                const unchanged = `${event.cancelBubble},${event.returnValue}`;
                event.cancelBubble = true;
                event.returnValue = false;
                const set = `${event.cancelBubble},${event.returnValue},${event.defaultPrevented}`;
                // `returnValue = false` on a non-cancelable event cancels nothing.
                const plain = new Event("ping");
                plain.returnValue = false;
                `${fresh}|${unchanged}|${set}|${plain.returnValue}`
                "#
            ),
            "false,true|false,true|true,false,true|true"
        );
    }

    #[test]
    fn init_event_re_initializes_an_event_but_not_during_a_dispatch() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const event = new Event("ping", { cancelable: true });
                event.preventDefault();
                event.initEvent("pong", true, false);
                const reset = `${event.type},${event.bubbles},${event.cancelable},${
                  event.defaultPrevented},${event.target}`;
                let midDispatch = "";
                target.addEventListener("pong", () => {
                  event.initEvent("nope");
                  midDispatch = event.type;
                });
                target.dispatchEvent(event);
                let arity = "no throw";
                try { event.initEvent(); } catch (error) { arity = error.constructor.name }
                `${reset}|${midDispatch}|${arity}`
                "#
            ),
            "pong,true,false,false,null|pong|TypeError"
        );
    }

    #[test]
    fn custom_event_carries_its_detail() {
        assert_eq!(
            trace(
                r#"
                const detail = { id: 1 };
                const event = new CustomEvent("ping", { detail, cancelable: true });
                const empty = new CustomEvent("ping");
                event.initCustomEvent("pong", true, false, "later");
                `${event.detail},${event.type},${empty.detail},${empty instanceof Event},${
                  Object.prototype.toString.call(empty)}`
                "#
            ),
            "later,pong,null,true,[object CustomEvent]"
        );
    }

    #[test]
    fn report_error_hands_the_value_to_the_realm_s_report_hook() {
        // The hook is what a worker realm replaces to fire its global `error`
        // event and escalate to its parent, so delegating to it is the whole
        // of "report the exception" — and `reportError` must read it at call
        // time, after that replacement.
        assert_eq!(
            trace(
                r#"
                const seen = [];
                __natives.reportException = (value) => seen.push(value);
                const failure = new TypeError("boom");
                reportError(failure);
                let arity = "no throw";
                try { reportError(); } catch (error) { arity = error.constructor.name }
                `${seen.length},${seen[0] === failure},${arity},${reportError.length}`
                "#
            ),
            "1,true,TypeError,1"
        );
    }
}
