//! WHATWG web-platform APIs for den: Blob/File/FileReader/FormData,
//! XMLHttpRequest, EventSource, URLPattern, compression streams and WebSocket.
//!
//! Classes are native `#[rquickjs::class]` types. EventTarget subclasses set
//! `[[Prototype]]` to `globalThis.EventTarget.prototype` at evaluate time so
//! `instanceof EventTarget` holds after `den:worker`.

pub mod blob;
pub mod compression;
pub mod events;
pub mod eventsource;
pub mod fetch;
pub mod file_reader;
pub mod form_data;
pub mod host;
pub mod streams;
mod url;
pub mod urlpattern;
pub mod websocket;
pub mod xhr;

#[rquickjs::module]
pub mod whatwg {
    use den_stdlib_worker::events::{Event, EventTarget, define_event_handler, define_on};
    use den_util::{ConstructorInstaller as _, inherit};
    use rquickjs::{Class, Ctx, Result, class::JsClass, function::Opt, module::Exports};

    use crate::host::Host;
    pub use crate::{
        blob::{Blob, File},
        compression::{CompressionStream, DecompressionStream},
        events::{CloseEvent, ProgressEvent},
        eventsource::EventSource,
        file_reader::FileReader,
        form_data::FormData,
        streams::{
            ByteLengthQueuingStrategy, CountQueuingStrategy, ReadableStream,
            ReadableStreamDefaultController, ReadableStreamDefaultReader, TransformStream,
            TransformStreamDefaultController, WritableStream, WritableStreamDefaultController,
            WritableStreamDefaultWriter,
        },
        urlpattern::URLPattern,
        websocket::WebSocket,
        xhr::XMLHttpRequest,
    };

    fn install_event_handlers<'js, C: JsClass<'js>>(ctx: &Ctx<'js>, names: &[&str]) -> Result<()> {
        let Some(proto) = Class::<C>::prototype(ctx)? else {
            return Ok(());
        };
        for name in names {
            define_event_handler(ctx.clone(), proto.clone(), (*name).to_owned(), Opt(None))?;
        }
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _exports: &Exports<'js>) -> Result<()> {
        let globals = ctx.globals();
        if !globals.contains_key("EventTarget")? {
            define_on(&globals)?;
        }
        globals.install_constructor::<Blob>(0)?;
        Class::<CloseEvent>::define(&globals)?;
        Class::<CompressionStream>::define(&globals)?;
        Class::<DecompressionStream>::define(&globals)?;
        Class::<EventSource>::define(&globals)?;
        Class::<File>::define(&globals)?;
        Class::<FileReader>::define(&globals)?;
        Class::<FormData>::define(&globals)?;
        Class::<ProgressEvent>::define(&globals)?;
        crate::streams::install_intrinsics(ctx)?;
        Class::<ByteLengthQueuingStrategy>::define(&globals)?;
        Class::<CountQueuingStrategy>::define(&globals)?;
        Class::<ReadableStream>::define(&globals)?;
        Class::<ReadableStreamDefaultController>::define(&globals)?;
        Class::<ReadableStreamDefaultReader>::define(&globals)?;
        Class::<TransformStream>::define(&globals)?;
        Class::<TransformStreamDefaultController>::define(&globals)?;
        Class::<WritableStreamDefaultController>::define(&globals)?;
        Class::<WritableStreamDefaultWriter>::define(&globals)?;
        let _ = Class::<crate::streams::ReadableStreamAsyncIterator>::create_constructor(ctx)?;
        Class::<crate::url::URL>::define(&globals)?;
        Class::<crate::url::URLSearchParams>::define(&globals)?;
        let _ = Class::<crate::url::UrlSearchIterator>::create_constructor(ctx)?;
        Class::<URLPattern>::define(&globals)?;
        Class::<WebSocket>::define(&globals)?;
        Class::<XMLHttpRequest>::define(&globals)?;
        Class::<WritableStream>::define(&globals)?;
        inherit::<File, Blob>(ctx)?;
        inherit::<ProgressEvent, Event>(ctx)?;
        inherit::<CloseEvent, Event>(ctx)?;
        inherit::<FileReader, EventTarget>(ctx)?;
        inherit::<XMLHttpRequest, EventTarget>(ctx)?;
        inherit::<EventSource, EventTarget>(ctx)?;
        inherit::<WebSocket, EventTarget>(ctx)?;
        install_event_handlers::<FileReader>(ctx, &[
            "onload",
            "onerror",
            "onloadend",
            "onloadstart",
            "onprogress",
            "onabort",
        ])?;
        install_event_handlers::<XMLHttpRequest>(ctx, &[
            "onreadystatechange",
            "onload",
            "onerror",
            "onloadend",
            "onloadstart",
            "onprogress",
            "onabort",
            "ontimeout",
        ])?;
        install_event_handlers::<EventSource>(ctx, &["onopen", "onmessage", "onerror"])?;
        install_event_handlers::<WebSocket>(ctx, &["onopen", "onmessage", "onerror", "onclose"])?;
        WebSocket::install_idl_constants(ctx)?;
        FileReader::install_idl_constants(ctx)?;
        Host::install_formdata_symbol(ctx)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
