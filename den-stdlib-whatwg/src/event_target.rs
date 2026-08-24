//! Listener maps for EventTarget subclasses. Prototype wiring makes
//! `instanceof EventTarget` hold; dispatch is native so it does not depend on
//! the worker WeakMap.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use rquickjs::{Ctx, Function, JsLifetime, Object, Result, Value, class::Trace, function::This};

use crate::host::Host;

#[derive(Trace, JsLifetime)]
struct Listener<'js> {
    callback: Value<'js>,
    capture:  bool,
    once:     bool,
    removed:  bool,
}

#[derive(Trace, JsLifetime)]
struct Handler<'js> {
    value:    Value<'js>,
    listener: Option<Value<'js>>,
}

#[derive(Trace, JsLifetime)]
pub struct HostEventTarget<'js> {
    listeners: HashMap<String, Vec<Listener<'js>>>,
    handlers:  HashMap<String, Handler<'js>>,
}

impl<'js> Default for HostEventTarget<'js> {
    fn default() -> Self {
        Self {
            listeners: HashMap::new(),
            handlers:  HashMap::new(),
        }
    }
}

pub type SharedEvents<'js> = Rc<RefCell<HostEventTarget<'js>>>;

impl<'js> HostEventTarget<'js> {
    pub fn new() -> Self { Self::default() }

    pub fn share() -> SharedEvents<'js> { Rc::new(RefCell::new(Self::new())) }

    /// Drop every listener and handler. Stored callbacks routinely capture
    /// their own target, and QuickJS cannot collect a cycle whose links are
    /// Rust-held refcounts, so a finished target must sever them itself or it
    /// leaks until the runtime's free assert fires.
    pub fn clear(&mut self) {
        self.listeners.clear();
        self.handlers.clear();
    }

    pub fn add(
        &mut self, ctx: &Ctx<'js>, type_: String, callback: Value<'js>, options: Option<Value<'js>>,
    ) -> Result<()> {
        if callback.is_null() || callback.is_undefined() {
            return Ok(());
        }
        if !callback.is_function() && !callback.is_object() {
            return Err(Host::throw_type(
                ctx,
                "addEventListener: callback is not an object",
            ));
        }
        let (capture, once, signal) = Self::flatten(options);
        if let Some(signal) = signal.as_ref() {
            if signal
                .as_object()
                .and_then(|obj| obj.get::<_, bool>("aborted").ok())
                .unwrap_or(false)
            {
                return Ok(());
            }
        }
        let list = self.listeners.entry(type_.clone()).or_default();
        let duplicate = list
            .iter()
            .any(|other| other.callback == callback && other.capture == capture);
        if duplicate {
            return Ok(());
        }
        list.push(Listener {
            callback,
            capture,
            once,
            removed: false,
        });
        let _ = signal;
        Ok(())
    }

    pub fn remove(&mut self, type_: &str, callback: &Value<'js>, options: Option<Value<'js>>) {
        let (capture, _, _) = Self::flatten(options);
        let Some(list) = self.listeners.get_mut(type_) else {
            return;
        };
        if let Some(index) = list
            .iter()
            .position(|other| other.callback == *callback && other.capture == capture)
        {
            list[index].removed = true;
            list.remove(index);
        }
    }

    pub fn dispatch_shared(
        this: &RefCell<Self>, ctx: &Ctx<'js>, target: &Object<'js>, event: Value<'js>,
    ) -> Result<bool> {
        let type_ = event
            .as_object()
            .and_then(|obj| obj.get::<_, String>("type").ok())
            .unwrap_or_default();
        let snapshot: Vec<Listener<'js>> = this
            .borrow()
            .listeners
            .get(&type_)
            .cloned()
            .unwrap_or_default();
        for record in snapshot {
            if record.removed {
                continue;
            }
            if record.once {
                this.borrow_mut().remove(&type_, &record.callback, None);
            }
            Self::invoke(ctx, target, &record.callback, event.clone());
        }
        Ok(true)
    }

    pub fn handler_or_null(&self, ctx: &Ctx<'js>, type_: &str) -> Value<'js> {
        self.handlers
            .get(type_)
            .map(|handler| handler.value.clone())
            .unwrap_or_else(|| Value::new_null(ctx.clone()))
    }

    pub fn set_handler(
        &mut self, ctx: &Ctx<'js>, target: Object<'js>, type_: &str, value: Value<'js>,
    ) -> Result<()> {
        let stored = if value.is_function() {
            value
        } else {
            Value::new_null(ctx.clone())
        };
        let existing = self
            .handlers
            .get(type_)
            .and_then(|handler| handler.listener.clone());
        if stored.is_null() {
            if let Some(listener) = existing {
                self.remove(type_, &listener, None);
                self.handlers.remove(type_);
            }
            return Ok(());
        }
        if existing.is_some() {
            if let Some(handler) = self.handlers.get_mut(type_) {
                handler.value = stored;
            }
            return Ok(());
        }
        let _ = target;
        let prop = format!("on{type_}");
        let wrapper = Function::new(ctx.clone(), {
            move |this: This<Object<'js>>, ctx: Ctx<'js>, event: Value<'js>| -> Result<()> {
                let callback: Value<'js> = this
                    .0
                    .get(&prop)
                    .unwrap_or_else(|_| Value::new_null(ctx.clone()));
                if let Some(func) = callback.as_function() {
                    let _ = func.call::<_, ()>((This(this.0.clone()), event));
                }
                Ok(())
            }
        })?;
        let wrapper_value = wrapper.clone().into_value();
        self.add(ctx, type_.to_string(), wrapper_value.clone(), None)?;
        self.handlers.insert(type_.to_string(), Handler {
            value:    stored,
            listener: Some(wrapper_value),
        });
        Ok(())
    }

    fn flatten(options: Option<Value<'js>>) -> (bool, bool, Option<Value<'js>>) {
        let Some(options) = options else {
            return (false, false, None);
        };
        if let Some(capture) = options.as_bool() {
            return (capture, false, None);
        }
        let Some(obj) = options.as_object() else {
            return (false, false, None);
        };
        let capture = obj.get::<_, bool>("capture").unwrap_or(false);
        let once = obj.get::<_, bool>("once").unwrap_or(false);
        let signal = obj
            .get::<_, Value>("signal")
            .ok()
            .filter(|value| !value.is_undefined() && !value.is_null());
        (capture, once, signal)
    }

    fn invoke(ctx: &Ctx<'js>, target: &Object<'js>, callback: &Value<'js>, event: Value<'js>) {
        let result = if let Some(func) = callback.as_function() {
            func.call::<_, ()>((This(target.clone()), event))
        } else if let Some(obj) = callback.as_object() {
            match obj.get::<_, Function>("handleEvent") {
                Ok(func) => func.call::<_, ()>((This(obj.clone()), event)),
                Err(error) => Err(error),
            }
        } else {
            Ok(())
        };
        if let Err(error) = result {
            Host::report_listener_error(ctx, error);
        }
    }
}

impl<'js> Clone for Listener<'js> {
    fn clone(&self) -> Self {
        Self {
            callback: self.callback.clone(),
            capture:  self.capture,
            once:     self.once,
            removed:  self.removed,
        }
    }
}
