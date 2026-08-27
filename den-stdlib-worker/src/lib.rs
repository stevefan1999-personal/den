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
        function::Opt,
        module::{Declarations, Exports},
        object::Property,
    };

    pub use super::{
        abort::{AbortController, AbortSignal},
        broadcast::BroadcastChannel,
        events::{
            CustomEvent, ErrorEvent, Event, EventTarget, MessageEvent, PromiseRejectionEvent,
        },
        navigator::NavigatorUAData,
        port::{MessageChannel, MessagePort},
        worker::Worker,
    };

    #[rquickjs::function(rename = "reportError")]
    #[qjs(rename = "reportError")]
    pub fn report_error<'js>(ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        super::events::report_error(ctx, value)
    }

    #[rquickjs::function(rename = "structuredClone")]
    #[qjs(rename = "structuredClone")]
    pub fn structured_clone<'js>(
        ctx: Ctx<'js>, value: Value<'js>, options: rquickjs::function::Opt<Value<'js>>,
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
        EventTarget::bind_on(ctx, &globals)?;
        crate::events::define_event_handler(
            ctx.clone(),
            globals.clone(),
            "onunhandledrejection".to_owned(),
            Opt(None),
        )?;
        crate::events::define_event_handler(
            ctx.clone(),
            globals.clone(),
            "onrejectionhandled".to_owned(),
            Opt(None),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn api_list_has_not_drifted() {
        assert_eq!(crate::API.len(), 17);
        assert_eq!(crate::API, [
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
        ]);
    }
}
