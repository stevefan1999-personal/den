# rquickjs macros, CLI shutdown, IndexMap, Temporal

Snapshot 2026-08-23. Sources: local `~/git/github.com/delskayn/rquickjs` (same 0.12 line den uses),
`~/git/github.com/boa-dev/temporal` (`temporal_rs` 0.2.6), den `5e72ab2`.

## 1. `#[qjs(declare)]` / `#[qjs(evaluate)]` — you almost never need them

`#[rquickjs::module]` already walks **pub** items and emits declare+export
(`rquickjs/macro/src/module/mod.rs`, `tests/macros/pass_module.rs`):

- `pub fn` with `#[rquickjs::function]` → exported function
- `pub const` / `pub static` → exported binding
- `pub use SuperClass` / `pub struct` with `#[rquickjs::class]` + constructor → exported class
- `#[qjs(skip)]` keeps a pub Rust item off the JS module

`#[qjs(declare)]` / `#[qjs(evaluate)]` exist only for **leftovers** the walker
cannot see (a stringly-named extra export). den currently re-lists every
function in both hooks, then sets the same names on `globalThis`. That is
duplicate of the macro.

**Pattern:**

```rust
#[rquickjs::module(rename_vars = "camelCase")]
pub mod timer {
    #[rquickjs::function]
    pub fn set_timeout(...) -> u32 { ... }

    // Only leftover: copy module exports onto globals.
    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let globals = ctx.globals();
        for name in ["setTimeout", "setInterval", "clearTimeout", "clearInterval"] {
            globals.set(name, exports.get::<_, Value>(name)?)?;
        }
        Ok(())
    }
}
```

Delete the `declare` functions. Delete `evaluate` that only `exports.export`s
what the macro already exported.

`#[rquickjs::bind(object)]` (`examples/native-module/using_macro.rs`) is the
other shape: a bag of functions as one object. Use it for `crypto.subtle`,
`process`, `WebAssembly` — not for a module of classes.

## 2. IndexMap does not remove `Ctx`

`IntoJs` for `IndexMap<K,V>` (`core/src/value/convert/into.rs`, `indexmap`
feature — den already enables it) still takes `&Ctx`. What it removes is
`Object::new(ctx)?; obj.set(...)?; obj.set(...)`.

```rust
// before
let object = Object::new(ctx.clone())?;
object.set("family", 4)?;
object.set("ip", ip)?;
Ok(object)

// after
Ok(indexmap! { "family" => 4.into_js(ctx)?, "ip" => ip.into_js(ctx)? }.into_js(ctx)?)
```

Use IndexMap for **plain dictionaries**. Keep `Object::new` only when you need
a prototype, a class instance, or property flags.

## 3. Ctrl-C / idle / promises — CancellationToken as a *JS type* is the bloat

rquickjs-cli (`examples/rquickjs-cli`) is a **sync** REPL with `AtomicBool`.
Useless for den's async loop.

Facts from rquickjs 0.12 `AsyncRuntime` (`core/src/runtime/async.rs`):

- `idle()` runs pending JS jobs **and** `ctx.spawn` futures until the
  scheduler is empty. A live timer/fetch/worker pump keeps `idle()` pending.
  That is Node's "handles keep the event loop alive".
- `idle()` **holds the runtime mutex the whole time**. The only way to do JS
  work during `idle()` is `ctx.spawn`.
- `drive()` polls the same scheduler but **releases the lock between polls**.
  den currently `tokio::spawn(runtime.drive())` *and* later `idle()`, so two
  loopers fight.
- Interrupt handler (`set_interrupt_handler`) fires on JS bytecode back-edges,
  not on a future parked in Tokio. A `sleep`ing timer is **not** interrupted
  until it calls back into JS.
- Dropping `idle()` does **not** cancel `ctx.spawn` futures.

So the Node/txiki behaviour (Ctrl-C stops everything) is:

1. **One** `AtomicBool` (or one internal `CancellationToken`, not JS-visible)
   that every `ctx.spawn` future selects on.
2. Interrupt handler reads the same flag (kills tight JS loops).
3. Event loop is **`runtime.idle()` only**. Do not also spawn `drive()`.
4. Ctrl-C sets the flag; spawned futures complete; `idle()` returns; process
   exits. No `run_until_cancelled(idle())` wrapper.

Timer `clearTimeout` must **not** return a JS `CancellationToken` class.
Browsers/Node use a numeric id. Store `HashMap<u32, CancellationToken>` (or
`AbortHandle`) in userdata.

Keep `tokio_util::sync::CancellationToken` as a **Rust** implementation
detail for those ids and for Engine shutdown. Delete
`den-stdlib-core::cancellation::CancellationTokenWrapper` from the JS
surface.

Workers still need a stop signal per realm; that can be the same engine flag
plus the existing worker registry `shutdown()`.

## 4. JS prelude → Rust classes

~3.6k lines of `include_str!("prelude/*.js")`. Reason they existed: rquickjs
`#[rquickjs::class]` cannot `extends` a JS class. If EventTarget is **also**
a Rust class, subclasses set the prototype:

```rust
if let (Some(sub), Some(event_target)) = (
    Class::<FileReader>::prototype(ctx)?,
    Class::<EventTarget>::prototype(ctx)?,
) {
    sub.set_prototype(Some(&event_target))?;
}
```

(`Object::set_prototype`, `Class::prototype` in `core/src/class.rs`,
`core/src/value/object.rs`). Then `fileReader instanceof EventTarget` holds.

EventTarget owns listener maps in the Rust struct (the JS WeakMaps in
`events.js`). Subclasses that need `dispatchEvent` either contain an
`EventTarget` or call the same methods on `self` after proto is wired.

Delete every `include_str!(...js)` after the Rust class has tests green.

## 5. Temporal

`temporal_rs` 0.2.6 is the Boa/Kiesel/V8 core. Types: `PlainDate`, `PlainTime`,
`PlainDateTime`, `ZonedDateTime`, `Instant`, `Duration`, `PlainYearMonth`,
`PlainMonthDay`, plus `Now`. Enable features `sys` + `compiled_data` for
host clock and tzdb.

Glue is a `den-stdlib-temporal` of `#[rquickjs::class]` wrappers, one class
per Temporal type, constructors calling `temporal_rs`. Do not reimplement
calendars.

test262: `test/built-ins/Temporal/**` (and optionally `test/intl402/Temporal`).
Harness: `test262/harness/assert.js`, `sta.js`, plus any `$262` host object
the tests call. A den runner evals harness + one test file through `Engine`.

Submodule path: `vendor/test262` → `https://github.com/tc39/test262`.
