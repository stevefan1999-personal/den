//! Web Workers API for den: `Worker`, `MessageChannel`/`MessagePort`,
//! `BroadcastChannel`, the `EventTarget` family, `AbortController`,
//! `performance`, `navigator` and `structuredClone`.
//!
//! Every JS-visible type is a `#[rquickjs::class]` / `#[rquickjs::function]`.
//! `evaluate` registers classes, reparents EventTarget subclasses, and copies
//! the exports onto `globalThis`.

pub mod abort;
pub mod broadcast;
pub mod events;
pub mod host;
pub mod message;
pub mod navigator;
pub mod performance;
pub mod port;
pub mod report;
pub mod transport;
pub mod worker;

pub use host::{BaseUrl, HostHandle, WorkerEngine, WorkerHost, WorkerHostError};
pub use message::{Message, throw_data_clone};
pub use port::NativePort;
pub use report::report_exception;
pub use transport::{Envelope, PortHandle};
pub use worker::RealmStop;

pub use crate::js_worker_module as js_worker;

/// Everything `den:worker` exports and installs as a global.
///
/// `DOMException` is deliberately absent: quickjs-ng registers it natively in
/// every context (`JS_AddIntrinsicAToB`), so there is nothing to install.
const API: [&str; 17] = [
    "AbortController",
    "AbortSignal",
    "BroadcastChannel",
    "CustomEvent",
    "ErrorEvent",
    "Event",
    "EventTarget",
    "MessageChannel",
    "MessageEvent",
    "MessagePort",
    "NavigatorUAData",
    "PromiseRejectionEvent",
    "Worker",
    "navigator",
    "performance",
    "reportError",
    "structuredClone",
];

/// The module definition itself. It is named for what it declares rather than
/// `worker`, which belongs to the file that spawns worker threads; the alias
/// below is the name embedders use.
#[rquickjs::module]
pub mod worker_module {
    use rquickjs::{
        Ctx, Function, Object, Result, Value,
        module::{Declarations, Exports},
        object::Property,
    };

    pub use super::abort::{AbortController, AbortSignal};
    pub use super::broadcast::BroadcastChannel;
    pub use super::events::{
        CustomEvent, ErrorEvent, Event, EventTarget, MessageEvent, PromiseRejectionEvent,
    };
    pub use super::navigator::NavigatorUAData;
    pub use super::port::{MessageChannel, MessagePort};
    pub use super::worker::Worker;

    #[rquickjs::function(rename = "reportError")]
    #[qjs(rename = "reportError")]
    pub fn report_error<'js>(ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        super::events::report_error(ctx, value)
    }

    #[rquickjs::function(rename = "structuredClone")]
    #[qjs(rename = "structuredClone")]
    pub fn structured_clone<'js>(
        ctx: Ctx<'js>,
        value: Value<'js>,
        options: rquickjs::function::Opt<Value<'js>>,
    ) -> Result<Value<'js>> {
        super::message::structured_clone(ctx, value, options)
    }

    #[qjs(declare)]
    pub fn declare(declare: &Declarations) -> Result<()> {
        // Instances, not constructors: the module macro cannot see them.
        declare.declare("navigator")?;
        declare.declare("performance")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let natives = Object::new(ctx.clone())?;
        crate::events::install(ctx, &natives)?;
        crate::message::install(ctx, &natives)?;
        crate::port::install(ctx, &natives)?;
        crate::worker::install(ctx, &natives)?;
        crate::broadcast::install(ctx, &natives)?;

        let namespace = exports.module().namespace()?;
        crate::events::finish(ctx, &namespace)?;
        crate::abort::finish(ctx)?;
        crate::port::finish(ctx)?;
        crate::worker::finish(ctx)?;
        crate::broadcast::finish(ctx)?;
        // Native functions have no `.prototype`; HTML `reportError` is an
        // ordinary function, and the surface test distinguishes those from
        // arrows by `typeof prototype === "object"`.
        let report_error: Function<'js> = namespace.get("reportError")?;
        report_error.set_name("reportError")?;
        let structured_clone: Function<'js> = namespace.get("structuredClone")?;
        structured_clone.set_name("structuredClone")?;
        if report_error
            .get::<_, Value<'js>>("prototype")?
            .is_undefined()
        {
            report_error.set("prototype", Object::new(ctx.clone())?)?;
        }

        let api = Object::new(ctx.clone())?;
        for name in crate::API {
            if name == "navigator" || name == "performance" {
                continue;
            }
            api.set(name, namespace.get::<_, Value>(name)?)?;
        }
        api.set(
            "performance",
            crate::performance::Performance::instance(ctx)?,
        )?;
        crate::navigator::install_navigator(ctx, &api)?;

        let globals = ctx.globals();
        for name in crate::API {
            let value: Value<'js> = api.get(name)?;
            if !value.is_undefined() {
                if name == "navigator" {
                    globals.prop(name, Property::from(value.clone()).enumerable())?;
                } else {
                    globals.set(name, value.clone())?;
                }
            }
            exports.export(name, value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rquickjs::{CatchResultExt, Context, FromJs, Module, Runtime};

    /// A realm with the **real** `den:worker` module evaluated, exactly as an
    /// embedder gets it: the natives, the globals and
    /// the module exports. Nothing here re-implements [`worker_module`].
    ///
    /// One runtime per test: the module keeps its clone hooks in the context
    /// userdata, so a context cannot be shared.
    fn realm() -> (Runtime, Context) {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let install = || {
                let (module, evaluated) =
                    Module::evaluate_def::<crate::js_worker, _>(ctx.clone(), "den:worker")?;
                evaluated.finish::<()>()?;
                // The module object itself is what the export assertions read,
                // so it is parked on the global under a name the module does not export.
                ctx.globals().set("moduleExports", module.namespace()?)
            };
            install()
                .catch(&ctx)
                .map_err(|error| error.to_string())
                .expect("den:worker evaluates");
        });
        (runtime, context)
    }

    fn eval<T>(source: &str) -> T
    where
        T: for<'js> FromJs<'js>,
    {
        let (_runtime, context) = realm();
        context
            .with(|ctx| {
                ctx.eval::<T, _>(source)
                    .catch(&ctx)
                    .map_err(|error| error.to_string())
            })
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn text(source: &str) -> String {
        eval::<String>(source)
    }

    /// Like [`text`], then drain microtasks so a script that parks its result
    /// on `globalThis.__result` from a `Promise.then` can still be read
    /// synchronously. Caps the drain so a runaway job queue cannot hang the
    /// test.
    fn text_jobs(source: &str) -> String {
        let (_runtime, context) = realm();
        context
            .with(|ctx| {
                ctx.eval::<(), _>(source)
                    .catch(&ctx)
                    .map_err(|error| error.to_string())?;
                for _ in 0..32 {
                    if !ctx.execute_pending_job() {
                        break;
                    }
                }
                ctx.globals()
                    .get::<_, String>("__result")
                    .catch(&ctx)
                    .map_err(|error| error.to_string())
            })
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// The whole of `den:worker`'s documented surface, spelled out here rather
    /// than read from [`API`]: a test that iterates the very list it is
    /// checking stops checking whatever that list loses.
    const DOCUMENTED: [&str; 17] = [
        "AbortController",
        "AbortSignal",
        "BroadcastChannel",
        "CustomEvent",
        "ErrorEvent",
        "Event",
        "EventTarget",
        "MessageChannel",
        "MessageEvent",
        "MessagePort",
        "NavigatorUAData",
        "PromiseRejectionEvent",
        "Worker",
        "navigator",
        "performance",
        "reportError",
        "structuredClone",
    ];

    /// Every documented name is exported *and* installed as a global, is the
    /// same object on both paths, and is of the kind its IDL says.
    ///
    /// Losing an entry of `API`, or letting `evaluate` stop exporting or stop
    /// installing globals, is invisible to a test that builds the API by hand —
    /// which is why this one goes through the module.
    #[test]
    fn den_worker_exports_and_installs_every_documented_name() {
        assert_eq!(crate::API, DOCUMENTED, "the API list and its tests drifted");
        let (_runtime, context) = realm();
        let report = context
            .with(|ctx| {
                let names = DOCUMENTED.join(",");
                ctx.eval::<Vec<String>, _>(format!(
                    r#"
                    "{names}".split(",").map((name) => {{
                      const exported = moduleExports[name];
                      const global = globalThis[name];
                      if (exported === undefined) return `${{name}}: not exported`;
                      if (exported !== global) return `${{name}}: global is not the export`;
                      // Instances (`performance`, `navigator`), not constructors.
                      const isObject = name === "performance" || name === "navigator";
                      if (isObject) {{
                        if (typeof exported !== "object" || exported === null) {{
                          return `${{name}}: not an object`;
                        }}
                        return `${{name}}: ok`;
                      }}
                      if (typeof exported !== "function") return `${{name}}: not a function`;
                      // Every class is constructible and carries a prototype
                      // object; `structuredClone` is the one plain function,
                      // and an arrow has no prototype at all.
                      const isClass = name !== "structuredClone";
                      const hasPrototype = typeof exported.prototype === "object";
                      if (isClass !== hasPrototype) return `${{name}}: wrong kind`;
                      if (exported.name !== name) return `${{name}}: named ${{exported.name}}`;
                      return `${{name}}: ok`;
                    }})
                    "#
                ))
                .catch(&ctx)
                .map_err(|error| error.to_string())
            })
            .expect("the surface check evaluates");
        let expected: Vec<String> = DOCUMENTED
            .iter()
            .map(|name| format!("{name}: ok"))
            .collect();
        assert_eq!(report, expected);
    }

    /// `DOMException` is quickjs-ng's, not ours (see [`API`]'s doc comment) —
    /// but the APIs throw it, so a build where it is missing has to fail
    /// loudly here rather than at the first failed clone.
    #[test]
    fn dom_exception_comes_from_the_engine_and_is_an_error() {
        assert_eq!(
            text(
                r#"(() => {
                     const error = new DOMException("why", "DataCloneError");
                     return [
                       typeof DOMException,
                       error instanceof Error,
                       error.name,
                       error.message,
                     ].join("|");
                   })()"#
            ),
            "function|true|DataCloneError|why"
        );
    }

    /// DOM §2.6: `stopPropagation()` stops the event travelling *between*
    /// targets, and with no tree that is nothing at all — the listeners already
    /// registered on this target still run. Only
    /// `stopImmediatePropagation()` cuts those short.
    #[test]
    fn stop_propagation_does_not_silence_the_other_listeners_of_the_target() {
        assert_eq!(
            text(
                r#"(() => {
                     const log = [];
                     const target = new EventTarget();
                     target.addEventListener("ping", (event) => {
                       event.stopPropagation();
                       log.push("first");
                     });
                     target.addEventListener("ping", () => log.push("second"));
                     const dispatched = target.dispatchEvent(new Event("ping"));
                     log.push(`dispatched:${dispatched}`);

                     const immediate = new EventTarget();
                     immediate.addEventListener("ping", (event) => {
                       event.stopImmediatePropagation();
                       log.push("only");
                     });
                     immediate.addEventListener("ping", () => log.push("never"));
                     immediate.dispatchEvent(new Event("ping"));
                     return log.join(",");
                   })()"#
            ),
            "first,second,dispatched:true,only"
        );
    }

    /// The flag is per dispatch, not per event: DOM §2.9 clears it on the way
    /// out, so the same event object re-dispatched is not still stopped.
    #[test]
    fn stop_immediate_propagation_is_cleared_when_the_dispatch_ends() {
        assert_eq!(
            text(
                r#"(() => {
                     const log = [];
                     const target = new EventTarget();
                     const event = new Event("ping");
                     let stop = true;
                     target.addEventListener("ping", (seen) => {
                       if (stop) seen.stopImmediatePropagation();
                       log.push("first");
                     });
                     target.addEventListener("ping", () => log.push("second"));
                     target.dispatchEvent(event);
                     stop = false;
                     target.dispatchEvent(event);
                     return log.join(",");
                   })()"#
            ),
            "first,first,second"
        );
    }

    /// `MessageEvent`'s three postMessage-between-origins members. They default
    /// to `""`/`""`/`null`, and a constructor that ignored the dictionary would
    /// leave them there.
    #[test]
    fn a_message_event_carries_a_non_default_origin_last_event_id_and_source() {
        assert_eq!(
            text(
                r#"(() => {
                     const source = new EventTarget();
                     const given = new MessageEvent("message", {
                       data: 1, origin: "https://den.example", lastEventId: "7", source,
                     });
                     const bare = new MessageEvent("message");
                     return [
                       given.origin, given.lastEventId, given.source === source,
                       `${bare.origin}|${bare.lastEventId}|${bare.source}`,
                       // Not a number: `origin` is a DOMString in the IDL.
                       new MessageEvent("message", { origin: 7 }).origin,
                     ].join(",");
                   })()"#
            ),
            "https://den.example,7,true,||null,7"
        );
    }

    /// DOM §2.7 step 2 and step 5: a listener whose signal is *already*
    /// aborted is never added, and aborting a signal afterwards removes the
    /// listener it was passed with — leaving every other listener alone.
    #[test]
    fn an_abort_signal_removes_a_listener_and_an_aborted_one_is_never_added() {
        assert_eq!(
            text(
                r#"(() => {
                     const log = [];
                     const target = new EventTarget();
                     const dead = new AbortController();
                     dead.abort();
                     target.addEventListener("ping", () => log.push("never"), { signal: dead.signal });

                     const live = new AbortController();
                     target.addEventListener("ping", () => log.push("aborted-later"), { signal: live.signal });
                     target.addEventListener("ping", () => log.push("unsignalled"));
                     target.dispatchEvent(new Event("ping"));
                     live.abort();
                     target.dispatchEvent(new Event("ping"));
                     return log.join(",");
                   })()"#
            ),
            "aborted-later,unsignalled,unsignalled"
        );
    }

    /// WinterTC AbortController / AbortSignal, translated from txiki's
    /// `test-abort-controller.js`. A comma-separated list of failed check
    /// names; empty means every assertion held.
    #[test]
    fn abort_controller_and_signal_follow_the_dom_abort_algorithm() {
        assert_eq!(
            text(
                r#"(() => {
                     const failed = [];
                     const check = (name, held) => { if (!held) failed.push(name); };

                     const fresh = new AbortController();
                     check("signalIsAbortSignal", fresh.signal instanceof AbortSignal);
                     check("signalIsEventTarget", fresh.signal instanceof EventTarget);
                     check("startsNotAborted", fresh.signal.aborted === false);
                     check("reasonUndefinedBeforeAbort", fresh.signal.reason === undefined);

                     const defaulted = new AbortController();
                     defaulted.abort();
                     check("abortedAfterAbort", defaulted.signal.aborted === true);
                     check("defaultReasonIsDOMException", defaulted.signal.reason instanceof DOMException);
                     check("defaultReasonIsAbortError", defaulted.signal.reason.name === "AbortError");

                     const custom = new AbortController();
                     const customReason = new Error("custom reason");
                     custom.abort(customReason);
                     check("abortedAfterCustomAbort", custom.signal.aborted === true);
                     check("customReasonPreserved", custom.signal.reason === customReason);

                     const once = new AbortController();
                     const first = new Error("first");
                     once.abort(first);
                     once.abort(new Error("second"));
                     check("abortIsIdempotent", once.signal.reason === first);

                     const listened = new AbortController();
                     let listenerCount = 0;
                     listened.signal.addEventListener("abort", () => { listenerCount++; });
                     let onabortCalled = false;
                     listened.signal.onabort = () => { onabortCalled = true; };
                     listened.abort();
                     listened.abort();
                     check("abortListenerFires", listenerCount === 1);
                     check("onabortFires", onabortCalled);

                     const live = new AbortController();
                     let threwWhileLive = false;
                     try { live.signal.throwIfAborted(); } catch { threwWhileLive = true; }
                     check("throwIfAbortedSilentWhenLive", !threwWhileLive);
                     const thrownReason = new Error("aborted!");
                     live.abort(thrownReason);
                     let threw = false;
                     try { live.signal.throwIfAborted(); } catch (error) {
                       threw = error === thrownReason;
                     }
                     check("throwIfAbortedThrowsReason", threw);

                     const preAborted = AbortSignal.abort();
                     check("staticAbortIsAborted", preAborted.aborted === true);
                     check("staticAbortDefaultIsDOMException", preAborted.reason instanceof DOMException);
                     check("staticAbortDefaultIsAbortError", preAborted.reason.name === "AbortError");
                     const staticReason = new Error("custom");
                     const preAbortedCustom = AbortSignal.abort(staticReason);
                     check("staticAbortPreservesReason", preAbortedCustom.reason === staticReason);

                     const firstSource = new AbortController();
                     const secondSource = new AbortController();
                     const combined = AbortSignal.any([firstSource.signal, secondSource.signal]);
                     check("anyStartsLive", combined.aborted === false);
                     let anyFired = false;
                     combined.addEventListener("abort", () => { anyFired = true; });
                     firstSource.abort(new Error("c1"));
                     check("anyAbortsWithSource", combined.aborted === true);
                     check("anyReasonMatchesSource", combined.reason.message === "c1");
                     check("anyAbortEventFired", anyFired);
                     secondSource.abort();
                     let anyCount = 0;
                     const a = new AbortController();
                     const b = new AbortController();
                     const onceCombined = AbortSignal.any([a.signal, b.signal]);
                     onceCombined.addEventListener("abort", () => { anyCount++; });
                     a.abort();
                     b.abort();
                     check("anyFiresOnlyOnce", anyCount === 1);

                     const already = new AbortController();
                     already.abort(new Error("already"));
                     const fromAborted = AbortSignal.any([already.signal]);
                     check("anyWithAbortedInputIsAborted", fromAborted.aborted === true);
                     check("anyWithAbortedInputKeepsReason", fromAborted.reason.message === "already");

                     const queued = [];
                     globalThis.setTimeout = (callback) => {
                       queued.push(callback);
                       return queued.length;
                     };
                     const timed = AbortSignal.timeout(50);
                     check("timeoutStartsLive", timed.aborted === false);
                     queued.forEach((callback) => callback());
                     check("timeoutAborts", timed.aborted === true);
                     check(
                       "timeoutReasonIsTimeoutError",
                       timed.reason instanceof DOMException && timed.reason.name === "TimeoutError",
                     );
                     delete globalThis.setTimeout;

                     check(
                       "controllerToStringTag",
                       Object.prototype.toString.call(new AbortController()) === "[object AbortController]",
                     );
                     check(
                       "signalToStringTag",
                       Object.prototype.toString.call(new AbortController().signal) === "[object AbortSignal]",
                     );

                     return failed.join(",");
                   })()"#
            ),
            ""
        );
    }

    /// High Resolution Time: `now()` is milliseconds since this realm's
    /// origin (monotonic), `timeOrigin` is that origin as Unix-epoch ms —
    /// not QuickJS-ng's monotonic reading. A busy-wait distinguishes an
    /// advancing clock from a frozen stub without needing `setTimeout`.
    #[test]
    fn performance_now_is_monotonic() {
        assert_eq!(
            text(
                r#"(() => {
                     const failed = [];
                     const check = (name, held) => { if (!held) failed.push(name); };
                     const start = performance.now();
                     check("nowIsNumber", typeof start === "number");
                     check("nowIsFinite", Number.isFinite(start));
                     check("nowIsNearOrigin", start >= 0 && start < 60000);
                     let later = start;
                     const deadline = Date.now() + 50;
                     while (later === start && Date.now() < deadline) later = performance.now();
                     check("nowAdvances", later > start);
                     check("nowIsMonotonic", performance.now() >= later);
                     check("timeOriginIsNumber", typeof performance.timeOrigin === "number");
                     check("timeOriginIsUnixEpoch", performance.timeOrigin > 1e12);
                     check(
                       "timeOriginNearWallClock",
                       Math.abs(Date.now() - performance.timeOrigin) < 60000,
                     );
                     check(
                       "toStringTag",
                       Object.prototype.toString.call(performance) === "[object Performance]",
                     );
                     return failed.join(",");
                   })()"#
            ),
            ""
        );
    }

    /// WinterTC `navigator.userAgentData`, translated from txiki's
    /// `test-navigator-useragentdata.js`. High-entropy values resolve through
    /// a Promise, so this test drains microtasks and reads `__result`.
    #[test]
    fn navigator_user_agent_data_reports_den() {
        assert_eq!(
            text_jobs(
                r#"
                globalThis.__result = "jobsDidNotRun";
                const failed = [];
                const check = (name, held) => { if (!held) failed.push(name); };
                const uad = navigator.userAgentData;
                check("userAgentDataExists", !!uad);
                check("isNavigatorUAData", uad instanceof NavigatorUAData);
                check(
                  "toStringTag",
                  Object.prototype.toString.call(uad) === "[object NavigatorUAData]",
                );
                check("brandsIsArray", Array.isArray(uad.brands) && uad.brands.length > 0);
                const brand = uad.brands[0];
                check("brandIsString", typeof brand.brand === "string");
                check("brandVersionIsString", typeof brand.version === "string");
                check("brandIsDen", brand.brand === "den");
                check("brandsFrozen", Object.isFrozen(uad.brands));
                check("brandEntryFrozen", Object.isFrozen(brand));
                check("mobileIsFalse", uad.mobile === false);
                check(
                  "platformIsKnown",
                  ["Linux", "macOS", "Windows", "FreeBSD", "OpenBSD"].includes(uad.platform)
                    || (typeof uad.platform === "string" && uad.platform.length > 0),
                );
                const json = uad.toJSON();
                check("toJSONBrands", Array.isArray(json.brands));
                check("toJSONMobile", json.mobile === uad.mobile);
                check("toJSONPlatform", json.platform === uad.platform);
                check("userAgentShape", /^den\/\d+\.\d+\.\d+/.test(navigator.userAgent));
                const major = navigator.userAgent.slice("den/".length).split(".")[0];
                check("brandVersionIsMajor", brand.version === major);
                check(
                  "hardwareConcurrency",
                  Number.isInteger(navigator.hardwareConcurrency) && navigator.hardwareConcurrency >= 1,
                );
                const descriptor = Object.getOwnPropertyDescriptor(globalThis, "navigator");
                check("navigatorEnumerable", descriptor.enumerable === true);
                check("navigatorNonWritable", descriptor.writable === false);
                check("navigatorNonConfigurable", descriptor.configurable === false);
                const before = navigator;
                try { navigator = {}; } catch { /* strict assignment throws */ }
                check("navigatorAssignmentIsIgnored", navigator === before);
                check(
                  "navigatorToStringTag",
                  Object.prototype.toString.call(navigator) === "[object Navigator]",
                );

                const highEntropy = uad.getHighEntropyValues([
                  "architecture", "bitness", "fullVersionList", "model",
                  "platformVersion", "wow64", "formFactors",
                ]);
                const empty = uad.getHighEntropyValues([]);
                const invalid = uad.getHighEntropyValues("not an array").then(
                  () => { check("invalidHintsShouldReject", false); },
                  (error) => { check("invalidHintsTypeError", error instanceof TypeError); },
                );
                Promise.all([highEntropy, empty, invalid]).then(([hev, none]) => {
                  check("hevHasBrands", Array.isArray(hev.brands));
                  check("hevHasMobile", typeof hev.mobile === "boolean");
                  check("hevHasPlatform", typeof hev.platform === "string");
                  check("hevArchitecture", typeof hev.architecture === "string");
                  check("hevBitness", typeof hev.bitness === "string");
                  check("hevFullVersionList", Array.isArray(hev.fullVersionList));
                  check("fullVersionListBrand", hev.fullVersionList[0].brand === "den");
                  check("fullVersionListHasDots", hev.fullVersionList[0].version.includes("."));
                  check("hevModel", typeof hev.model === "string");
                  check("hevPlatformVersion", typeof hev.platformVersion === "string");
                  check("platformVersionParts", hev.platformVersion.split(".").length >= 3);
                  check("hevWow64", typeof hev.wow64 === "boolean");
                  check("hevFormFactors", Array.isArray(hev.formFactors));
                  check("emptyStillHasBrands", Array.isArray(none.brands));
                  check("emptyHasNoArchitecture", none.architecture === undefined);
                  globalThis.__result = failed.join(",");
                }, (error) => {
                  globalThis.__result = `hevFailed:${error}`;
                });
                "#
            ),
            ""
        );
    }

    /// WebIDL: an interface object without a `Symbol.toStringTag` of its own
    /// makes `Object.prototype.toString.call(instance)` read `[object Object]`,
    /// which is what most of these used to say. The tag is read off each
    /// prototype: `MessagePort` cannot be constructed at all and the rest would
    /// need a runtime, and the tag lives on the prototype either way.
    #[test]
    fn every_platform_class_brands_itself_with_a_to_string_tag() {
        const BRANDED: [&str; 13] = [
            "AbortController",
            "AbortSignal",
            "Event",
            "CustomEvent",
            "MessageEvent",
            "ErrorEvent",
            "PromiseRejectionEvent",
            "EventTarget",
            "MessagePort",
            "MessageChannel",
            "Worker",
            "BroadcastChannel",
            "NavigatorUAData",
        ];
        let names = BRANDED.join(",");
        let report = eval::<Vec<String>>(&format!(
            r#"
            (() => {{
              const tagOf = (object) =>
                Object.prototype.toString.call(object).slice("[object ".length, -1);
              return [
                ..."{names}".split(",").map((name) =>
                  `${{name}}:${{tagOf(globalThis[name].prototype)}}`),
                // An actual instance, to prove the tag is inherited rather than
                // merely present on the prototype object.
                `instance:${{tagOf(new Event("x"))}}`,
              ];
            }})()
            "#
        ));
        let expected: Vec<String> = BRANDED
            .iter()
            .map(|name| format!("{name}:{name}"))
            .chain(["instance:Event".to_owned()])
            .collect();
        assert_eq!(report, expected);
    }

    /// HTML §8.1.7.5. den-core builds exactly this shape
    /// (`Engine::fire_rejection_event`): `new PromiseRejectionEvent(kind, {
    /// promise, reason, cancelable })`, dispatched at the global, with a
    /// cancelled dispatch meaning "handled". `promise` is a required init
    /// member, so the one-argument form is a TypeError.
    #[test]
    fn a_promise_rejection_event_carries_its_promise_and_reason() {
        assert_eq!(
            text(
                r#"(() => {
                     const promise = Promise.resolve();
                     const reason = new Error("boom");
                     const event = new PromiseRejectionEvent("unhandledrejection", {
                       promise, reason, cancelable: true,
                     });
                     let arity = "accepted";
                     try { new PromiseRejectionEvent("unhandledrejection") }
                     catch (error) { arity = error.constructor.name }
                     const target = new EventTarget();
                     target.addEventListener("unhandledrejection", (seen) => seen.preventDefault());
                     return [
                       event.type,
                       event.promise === promise,
                       event.reason === reason,
                       event.cancelable,
                       arity,
                       // The cancellation protocol den-core reads.
                       target.dispatchEvent(event),
                     ].join(",");
                   })()"#
            ),
            "unhandledrejection,true,true,true,TypeError,false"
        );
    }
}
