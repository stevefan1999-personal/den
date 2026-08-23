# rquickjs 0.8.1 → 0.12.2 migration (den)

Status: research complete, implementation pending.
All claims below were verified against the local crate sources:

- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-0.12.2/`
- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-core-0.12.2/`
- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-macro-0.12.2/`
- diffed against the 0.8.1 counterparts (all three are present locally)

and against a real `cargo check` run of the workspace with the already-bumped
`Cargo.toml` (see [Appendix A](#appendix-a--actual-compiler-output)).

A second, independent verification pass has since corrected several line
references and error counts and added three missing findings — read the
[Verification log](#verification-log) at the bottom before trusting any
line number in this document.

---

## 0. TL;DR — what actually breaks

Three rquickjs changes break den's build and one breaks it silently at
runtime. Everything else is additive, deprecation, or feature-list hygiene.

| # | Change | den files |
|---|--------|-----------|
| 1 | `Loader::load` / `Resolver::resolve` gained an `attributes` parameter | `den-core/src/loader/http.rs`, `den-core/src/loader/mmap_script.rs`, `den-core/src/resolver/http.rs` |
| 2 | `#[rquickjs::class]` now **rejects** fields whose type is itself a `JsClass` | `den-stdlib-wasm/src/lib.rs` (`ResultObject`) |
| 3 | `async_with!` is `#[deprecated]` (still works) | `den-core/src/engine.rs`, `src/app.rs`, `src/main.rs` |
| 4 | **No compile error:** new `Type::Proxy` variant makes `console.log(proxy)` print nothing (§9a) | `den-stdlib-console/src/lib.rs:135` |

The state of the repo when this was written: `Cargo.toml` / `Cargo.lock` are
**already** on `rquickjs 0.12.2`; the `.rs` files are still 0.8-era. So this doc
is a to-do list, not a plan.

There is a **fourth** change that breaks nothing at compile time but breaks
`console.log` at runtime: `Value::type_of()` now returns the new `Type::Proxy`
for proxies instead of `Type::Object` (§9a). Fix it in the same pass.

> Note: the same working tree also bumped `derive_more 1.0 → 2.1`,
> `wasmtime 27 → 48`, `wasmi 0.40 → 1.1`, dropped `getset`/`wabt`/`derivative`,
> and renamed `den-transpiler-swc → den-transpiler-oxc`. Most compiler errors
> you will see come from **those**, not from rquickjs. Section 12 lists which is
> which so you don't chase the wrong crate. **Start with §12.0** —
> `den-transpiler-oxc` currently fails to compile, which blocks `den-core`,
> `den`, and any attempt to verify the §2/§4 edits.

---

## 1. den's rquickjs surface (inventory)

`grep -rn "rquickjs" --include='*.rs'` → 169 hits across 32 files.

| Area | Files |
|---|---|
| Runtime/context/eval | `den-core/src/engine.rs`, `src/app.rs`, `src/main.rs` |
| Custom `Loader` | `den-core/src/loader/http.rs`, `den-core/src/loader/mmap_script.rs` |
| Custom `Resolver` | `den-core/src/resolver/http.rs` |
| Modules (`#[rquickjs::module]`) | `den-stdlib-core`, `den-stdlib-console`, `den-stdlib-text`, `den-stdlib-timer`, `den-stdlib-crypto`, `den-stdlib-fs`, `den-stdlib-networking`, `den-stdlib-sqlite`, `den-stdlib-whatwg-fetch`, `den-stdlib-wasm` |
| Classes (`#[rquickjs::class]` + `#[rquickjs::methods]`) | `den-stdlib-core/src/cancellation.rs`, `den-stdlib-console/src/lib.rs`, `den-stdlib-text/src/lib.rs`, `den-stdlib-networking/src/{ip_addr,socket_addr,socket}.rs`, `den-stdlib-sqlite/src/lib.rs`, `den-stdlib-whatwg-fetch/src/lib.rs`, all of `den-stdlib-wasm/src/*` |
| Manual `JsLifetime` | `den-stdlib-wasm/src/store.rs:16-18` |
| `Ctx::userdata` / `store_userdata` | `den-stdlib-wasm/src/{lib,memory,module,instance,global,table}.rs` |
| `Persistent` + `function::Args` | `den-stdlib-wasm/src/instance.rs` |
| Manual `FromJs`/`IntoJs` | `den-utils/src/serde_json.rs`, `den-stdlib-wasm/src/{utils,memory,global,table}.rs` |

---

## 2. BREAKING #1 — `Loader` / `Resolver` gained import attributes

`rquickjs-core-0.12.2/src/loader.rs:64-71` and `:97-104`. Backing change:
"Added import attributes to the Loader trait #601" (`rquickjs-0.12.2/CHANGELOG.md:47`).

New trait shapes (`rquickjs-core-0.12.2/src/loader.rs:64` and `:99`):

```rust
pub trait Resolver {
    fn resolve<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        attributes: Option<ImportAttributes<'js>>,   // NEW
    ) -> Result<String>;
}

pub trait Loader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        attributes: Option<ImportAttributes<'js>>,   // NEW
    ) -> Result<Module<'js, Declared>>;
}
```

`ImportAttributes<'js>` is a newtype over `Object<'js>` with three methods
(`rquickjs-core-0.12.2/src/loader.rs:74-91`):

```rust
impl<'js> ImportAttributes<'js> {
    pub fn get(&self, key: &str) -> Result<Option<String>>;
    pub fn get_type(&self) -> Result<Option<String>>;      // shorthand for get("type")
    pub fn keys(&self) -> ObjectKeysIter<'js, String>;
}
```

It is `Clone` (needed because the tuple impls fan it out to every member;
see `loader.rs:257` `_attributes.clone()`). Tuple impls still go up to 8
elements (`loader.rs:308` `loader_impls!(A B C D E F G H)`), so den's
3-resolver / 4-loader tuples in `den-core/src/engine.rs:44-222` are fine.

Under the hood 0.12 switched to `JS_SetModuleLoaderFunc2` +
`JS_SetModuleNormalizeFunc2` (`loader.rs:134-142`); nothing den-visible.

### 2a. `den-core/src/loader/http.rs`

BEFORE (`den-core/src/loader/http.rs:4,25`):

```rust
use rquickjs::{loader::Loader, module::Declared, Ctx, Error, Module, Result};

impl Loader for HttpLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> Result<Module<'js, Declared>> {
```

AFTER:

```rust
use rquickjs::{
    loader::{ImportAttributes, Loader},
    module::Declared,
    Ctx, Error, Module, Result,
};

impl Loader for HttpLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
```

Body is unchanged.

### 2b. `den-core/src/loader/mmap_script.rs`

Identical edit at `den-core/src/loader/mmap_script.rs:4,40`.

### 2c. `den-core/src/resolver/http.rs`

BEFORE (`den-core/src/resolver/http.rs:2,12`):

```rust
use rquickjs::{loader::Resolver, Ctx, Error, Result};

impl Resolver for HttpResolver {
    fn resolve(&mut self, _ctx: &Ctx<'_>, base_path: &str, path: &str) -> Result<String> {
```

AFTER — note the method must become generic over `'js` because the new
parameter mentions it:

```rust
use rquickjs::{
    loader::{ImportAttributes, Resolver},
    Ctx, Error, Result,
};

impl Resolver for HttpResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base_path: &str,
        path: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
```

Body is unchanged.

### 2d. Optional follow-up (not required to compile)

`HttpLoader` currently sniffs `Content-Type` to decide js-vs-ts
(`den-core/src/loader/http.rs:30-76`). `import x from "…" with { type: "…" }`
is now available and is the standards-track way to say it. Cheapest useful
upgrade: let an explicit `type` attribute win over the MIME sniff, otherwise
keep today's behaviour.

---

## 3. BREAKING #2 — `JsClass`-typed fields are now a compile error

`rquickjs-macro-0.12.2/src/fields.rs:266-279` injects a check into every
generated `#[qjs(get)]`/`#[qjs(set)]` accessor; the check itself is
`rquickjs-core-0.12.2/src/class/impl_.rs:79-124` (`JsClassFieldCheck` +
`NotAJsClassField`, autoref-specialisation with a
`#[diagnostic::on_unimplemented]` message).

Rationale from `rquickjs-0.12.2/CHANGELOG.md:85`: the generated getter
**cloned** the value, so nested mutations through the returned JS object were
silently dropped. Issue #532.

den hits it exactly once, twice over, in `den-stdlib-wasm/src/lib.rs:31-38`
(inside the `#[rquickjs::module] pub mod wasm { … }` block that starts at
`lib.rs:11`):

BEFORE:

```rust
#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class()]
pub struct ResultObject {
    #[qjs(get, enumerable)]
    pub module:   crate::module::Module,     // Module is #[rquickjs::class]
    #[qjs(get, enumerable)]
    pub instance: crate::instance::Instance, // Instance is #[rquickjs::class]
}
```

Actual error (verified, see Appendix A):

```
error[E0277]: using a `JsClass` type directly as a class field is not supported
  --> den-stdlib-wasm/src/lib.rs:33:5
   | `module::Module` implements `JsClass` — wrap the field in `Class<'js, T>` instead
   = note: nested mutations are lost because the generated getter clones the value
```

AFTER — wrap in `Class<'js, T>`, which forces a `'js` parameter onto
`ResultObject` and therefore onto the `instantiate` return type.

`Class` must be added to the **inner** module's import list at
`den-stdlib-wasm/src/lib.rs:17-20` (the `use rquickjs::{…}` inside
`pub mod wasm`), not to the file header — the file header has no `rquickjs`
import at all.

```rust
// den-stdlib-wasm/src/lib.rs:17-20, inside `pub mod wasm`
use rquickjs::{
    class::Trace, module::Exports, prelude::Opt, ArrayBuffer, Class, Ctx, Exception,
    IntoJs, JsLifetime, Result, TypedArray, Value,
};

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class()]
pub struct ResultObject<'js> {
    #[qjs(get, enumerable)]
    pub module:   Class<'js, crate::module::Module>,
    #[qjs(get, enumerable)]
    pub instance: Class<'js, crate::instance::Instance>,
}

#[rquickjs::methods]
impl<'js> ResultObject<'js> {
    #[qjs(constructor)]
    pub fn new() {}
}

#[rquickjs::function]
pub async fn instantiate<'js>(
    module_or_buffer_source: Either<Module, Either<TypedArray<'js, u8>, ArrayBuffer<'js>>>,
    import_object: Opt<IndexMap<String, IndexMap<String, Value<'js>>>>,
    ctx: Ctx<'js>,
) -> Result<ResultObject<'js>> {
    let module = match module_or_buffer_source {
        Either::Left(module) => module,
        Either::Right(buffer_source) => Module::new2(buffer_source, &ctx)?,
    };
    let instance = Instance::new(&module, import_object, ctx.clone())?;
    Ok(ResultObject {
        module:   Class::instance(ctx.clone(), module)?,
        instance: Class::instance(ctx, instance)?,
    })
}
```

`Class::instance(ctx, value) -> Result<Class<'js, C>>` is unchanged from 0.8
(`rquickjs-core-0.12.2/src/class.rs:223`). `Class<'js, C>` implements
`Trace` and `JsLifetime`, so the derives still work; the `JsLifetime` derive
handles the new `'js` parameter automatically
(`rquickjs-macro-0.12.2/src/js_lifetime.rs:167-177`).

`Instance::new` currently takes `ctx: Ctx<'js>` by value
(`den-stdlib-wasm/src/instance.rs:212-216`), hence the `ctx.clone()` above.
The current call is `Instance::new(&module, import_object, ctx)?` at
`den-stdlib-wasm/src/lib.rs:55`.

**Scope check:** this is the *only* place in den where a `#[qjs(get)]` field
has a `JsClass` type. Every other `#[qjs(get, …)]` in den is either on a plain
data field or on a *method* — the check only fires on **fields**. The full list
of `#[qjs(get…)]` sites in den (`grep -rn '#\[qjs(get'`):

| Site | Kind | JsClass type? |
|---|---|---|
| `den-stdlib-wasm/src/lib.rs:35,37` | **field** | **yes → must fix** |
| `den-stdlib-networking/src/ip_addr.rs:18,22,26,30,34` | method | no |
| `den-stdlib-networking/src/socket_addr.rs:20,28,32` | method | no |
| `den-stdlib-networking/src/socket_addr.rs:36` (`ip`) | method | **yes** (`IpAddrWrapper`) — see below |
| `den-stdlib-networking/src/socket.rs:29,83` | method | no |
| `den-stdlib-text/src/lib.rs:45,50,55,125` | field / method | no |
| `den-stdlib-wasm/src/memory.rs:71` (`buffer`) | method | no |
| `den-stdlib-wasm/src/instance.rs:229` (`exports`) | method | no |

`den-stdlib-text`, `den-stdlib-console`, `den-stdlib-networking`,
`den-stdlib-core`, `den-stdlib-crypto`, `den-stdlib-fs`, `den-stdlib-sqlite`,
`den-stdlib-timer`, `den-stdlib-whatwg-fetch` and `den-utils` all
`cargo check` **clean** against 0.12.2 today — see the Verification log.

### 3a. The same hazard exists on getter *methods*, silently

`den-stdlib-networking/src/socket_addr.rs:36-38`:

```rust
#[qjs(get, rename = "ip", enumerable)]
#[delegate(self.addr)]
#[into]
pub fn ip(&self) -> IpAddrWrapper {}
```

`IpAddrWrapper` is `#[rquickjs::class(rename = "IpAddr")]`. Returning a
`JsClass` **by value from a getter method** has exactly the clone semantics
that issue #532 is about — `sockAddr.ip.someField = x` mutates a throwaway
instance — but `JsClassFieldCheck` is only injected by
`rquickjs-macro-0.12.2/src/fields.rs:266-279` for **fields**, so this compiles
silently. Not a build break, not required for the bump; just do not "fix" the
field check by converting `ResultObject`'s fields into methods, because that
merely hides the same bug.

---

## 4. BREAKING #3 (soft) — `async_with!` is deprecated

`rquickjs-core-0.12.2/src/context/async.rs:70-71` — the macro carries
`#[deprecated]`. It still expands and works; it now just forwards to the async
closure form (`context/async.rs:73-77`):

```rust
macro_rules! async_with{
    ($context:expr => |$ctx:ident| { $($t:tt)* }) => {
        $crate::AsyncContext::async_with(&$context, async |$ctx| { $($t)* })
    };
}
```

The signature change that made it redundant (`context/async.rs:216-222`):

```rust
// 0.8.1
pub fn async_with<F, R>(&self, f: F) -> WithFuture<F, R>
where
    F: for<'js> FnOnce(Ctx<'js>) -> Pin<Box<dyn Future<Output = R> + 'js + Send>> + ParallelSend,
    R: ParallelSend;

// 0.12.2
pub fn async_with<F, R>(&self, f: F) -> WithFuture<F, R>
where
    F: for<'js> AsyncFnOnce(Ctx<'js>) -> R + ParallelSend,
    R: ParallelSend;
```

`WithFuture` still does the `Box::pin` internally
(`rquickjs-core-0.12.2/src/context/async/future.rs:40`), so there is no
behavioural difference — the old macro's hand-rolled `uplift` transmute is
simply gone.

den has **four** invocation sites across three files, plus three `use` imports
(`grep -rn async_with --include='*.rs'`):

| Site | Form |
|---|---|
| `den-core/src/engine.rs:5` | `use rquickjs::{async_with, …}` |
| `den-core/src/engine.rs:313` | `async_with!(self.context => \|ctx\| { … })` |
| `den-core/src/engine.rs:359` | `async_with!(self.context => \|ctx\| { … })` |
| `src/app.rs:3` | `use rquickjs::{async_with, convert::Coerced}` |
| `src/app.rs:51` | `async_with!(engine.context => \|ctx\| { … })` |
| `src/main.rs:6` | `use rquickjs::{async_with, Coerced}` |
| `src/main.rs:53` | `async_with!(app.engine.context => \|ctx\| { … })` |

All four invocations become plain method calls.

`den-core/src/engine.rs:313`:

```rust
// BEFORE
Ok(async_with!(self.context => |ctx| {
    let src = format!(r#"await import(`{}`)"#, filename.to_str().unwrap());
    ctx.eval_with_options::<Promise, _>(src, { … })?
       .into_future::<Object>().await?.get("value")
})
.await?)

// AFTER
Ok(self.context.async_with(async |ctx| {
    let src = format!(r#"await import(`{}`)"#, filename.to_str().unwrap());
    ctx.eval_with_options::<Promise, _>(src, { … })?
       .into_future::<Object>().await?.get("value")
})
.await?)
```

Same edit at `den-core/src/engine.rs:359` (`Engine::eval`), `src/app.rs:51`
and `src/main.rs:53`. For the latter two note the receiver form —
`async_with!(engine.context => …)` becomes `engine.context.async_with(async |ctx| …)`.

Drop `async_with` from the `use rquickjs::{…}` lists in
`den-core/src/engine.rs:5`, `src/app.rs:3`, `src/main.rs:6`.

MSRV: async closures need Rust 1.85+. den declares `rust-version = "1.97"` and
`rust-toolchain.toml` pins `stable`, so this is free. rquickjs 0.12's own MSRV
is 1.87 (`CHANGELOG.md:70`).

---

## 5. Macro attributes — everything den uses still works

Read `rquickjs-macro-0.12.2/src/common.rs:120-149` (`mod kw`) for the full
accepted keyword set. Verified by compiling `den-stdlib-text` and
`den-stdlib-console` clean against 0.12.2.

| Attribute | 0.12.2 status | Source |
|---|---|---|
| `#[rquickjs::class]`, `class(rename = "…")`, `class(rename_all = …)`, `class(frozen)` | unchanged | `rquickjs-macro-0.12.2/src/class.rs:18-24` |
| `#[rquickjs::class(exotic)]` | **new** flag | `class.rs:20,38` |
| `#[rquickjs::methods]`, `methods(rename_all = "camelCase")` | unchanged | `rquickjs-macro-0.12.2/src/methods.rs` |
| `#[qjs(constructor)]`, `#[qjs(static)]`, `#[qjs(skip)]`, `#[qjs(get)]`, `#[qjs(set)]`, `#[qjs(rename = …)]`, `#[qjs(enumerable)]`, `#[qjs(configurable)]` | unchanged | `rquickjs-macro-0.12.2/src/methods/method.rs:16-30,62-105` |
| `#[qjs(prop)]`, `#[qjs(writable)]` | **new** | `method.rs:26,29,72,74` |
| `#[qjs(skip_trace)]` | unchanged | `rquickjs-macro-0.12.2/src/trace.rs:148` |
| `#[rquickjs::module(rename/rename_vars/rename_types)]` | unchanged | `rquickjs-macro-0.12.2/src/module/mod.rs` |
| `#[qjs(declare)]`, `#[qjs(evaluate)]` | unchanged | `common.rs:141-142` |
| `#[rquickjs::function]`, `function(rename = "…")` | unchanged | `rquickjs-macro-0.12.2/src/function.rs` (only visibility of internals changed) |
| `#[derive(Trace)]`, `#[derive(JsLifetime)]` | unchanged | `trace.rs`, `js_lifetime.rs` |
| `#[derive(FromJs)]`, `#[derive(IntoJs)]` | **new** derives, re-exported at root | `rquickjs-0.12.2/src/lib.rs:127-129` |

### 5a. New hard rule: `#[rquickjs::class]` + `#[derive(FromJs/IntoJs)]` is now an error

`rquickjs-macro-0.12.2/src/class.rs:351,499-529` — `ensure_no_conflicting_derives`
rejects `FromJs`/`IntoJs` in the derive list of a `#[class]` type (including
path-qualified `rquickjs::FromJs`). den does not do this today; just don't reach
for the new derives on class types.

### 5b. Behaviour change: prototype methods are no longer enumerable

`rquickjs-macro-0.12.2/src/methods/method.rs:296-315`. 0.8 installed methods
with `_proto.set(name, f)`; 0.12 uses:

```rust
_proto.prop(name, Property::from(f).writable().configurable())
```

`Property` defaults to non-enumerable, so `for (const k in obj)` no longer walks
into prototype methods. This matches spec (`class` methods are non-enumerable)
but it is JS-visible. Anything in den's JS/TS test surface that enumerated e.g.
`Response.prototype` members will change. Nothing in den's Rust code depends on it.

### 5c. Behaviour change: `#[qjs(static, get/set)]` accessors moved to the constructor

`rquickjs-macro-0.12.2/src/methods.rs:224-227,248-250` — static accessors are
now installed on the constructor object instead of the prototype (issue #478),
and a getter/setter pair must agree on `static`
(`rquickjs-macro-0.12.2/src/methods/accessor.rs:33-42,57-66`). den has no
`#[qjs(static, get)]`, so no action.

### 5d. Behaviour change: module auto-declarations now use the JS name

`rquickjs-macro-0.12.2/src/module/mod.rs:358,365` — the auto-generated
`Declarations::declare` key changed from the Rust ident to the JS name.

Previously (0.8) `#[rquickjs::function(rename = "setInterval")]` inside a
`#[rquickjs::module]` declared `"set_interval"` but exported `"setInterval"` —
i.e. the export never matched a declaration. 0.12 fixes that. Consequence for
`den-stdlib-timer/src/lib.rs:77-84`: the hand-written

```rust
#[qjs(declare)]
pub fn declare(declare: &Declarations) -> Result<()> {
    declare.declare("setInterval")?;
    …
}
```

is now redundant (the macro declares the same names). Duplicate
`JS_AddModuleExport` is harmless (`rquickjs-core-0.12.2/src/value/module.rs:142-145`
is a bare FFI add), so this is cleanup, not a break.

The five `#[qjs(declare)]` blocks in den, and whether each is still needed:

| Block | Still required? |
|---|---|
| `den-stdlib-timer/src/lib.rs:77-84` | no — the four names are auto-declared now |
| `den-stdlib-core/src/lib.rs:54` | yes — declares `super::`-defined functions |
| `den-stdlib-crypto/src/lib.rs:47` | yes — same |
| `den-stdlib-fs/src/lib.rs:9` | yes — same |
| `den-stdlib-whatwg-fetch/src/lib.rs:169-173` | yes — declares `fetch`, defined at `super::js_fetch` |

---

## 6. `JsLifetime` — trait unchanged, manual impls still legal

Trait definition is byte-identical
(`rquickjs-core-0.12.2/src/js_lifetime.rs:79-82`):

```rust
pub unsafe trait JsLifetime<'js> {
    type Changed<'to>: 'to;
}
```

Additions only (`js_lifetime.rs:96-186`): `CString`, `Proxy`, and — new — a
blanket `unsafe impl<'js> JsLifetime<'js> for ()` at `js_lifetime.rs:188`. The
`impl_outlive!` list moved from `std::` to `alloc::`/`core::` paths (no
semantic change) and the `std`-only entries are behind `#[cfg(feature = "std")]`.

`Ctx<'js>` does **not** implement `JsLifetime` — in either version. That is
precisely why den needs the manual impl in
`den-stdlib-wasm/src/store.rs:16-18`; the `#[derive(JsLifetime)]` macro emits a
`ValidJsLifetimeImpl` bound requiring every `'js`-carrying field type to
implement `JsLifetime<'js>` (`rquickjs-macro-0.12.2/src/js_lifetime.rs:159-176`),
which `Ctx<'js>` fails. That manual impl still compiles in 0.12.

One macro fix worth knowing: 0.12 namespace-qualifies the emitted bound
(`js_lifetime.rs:168`, `#crate_name::JsLifetime<#lt>` instead of bare
`JsLifetime<#lt>`), so `#[derive(JsLifetime)]` no longer requires
`use rquickjs::JsLifetime` to be in scope
(`rquickjs-0.12.2/CHANGELOG.md:143`, PR #429).

See section 10 for why den should delete that manual impl anyway.

---

## 7. `class::Trace` — additive only

`rquickjs-core-0.12.2/src/class/trace.rs`. New `Trace` impls: `Atom<'js>`
(:105), `Constructor<'js>` (:111), `Proxy<'js>` (:117), `TypedArray<'js, T>`
(:123), plus `Promise` and `ArrayBuffer` added to the `trace_impls!` list (:248-249).

Practical effect for den: fields of type `TypedArray`/`ArrayBuffer`/`Promise`
no longer need `#[qjs(skip_trace)]`. None of den's classes currently hold one,
so nothing to change. Existing `#[qjs(skip_trace)]` on foreign types
(`wasmtime::Memory`, `rusqlite::Connection`, `CancellationToken`,
`&'static Encoding`, `reqwest::Response`, …) is still required and still works.

Note `crate::Atom<'js>` moved out of the by-value `trace_impls!` list into a
dedicated impl that marks the ctx (`trace.rs:102-106`) — no den impact.

---

## 8. `Ctx::userdata` / `store_userdata` — unchanged

`rquickjs-core-0.12.2/src/context/ctx.rs:480,492,503`:

```rust
pub fn store_userdata<U>(&self, data: U) -> StdResult<Option<Box<U>>, UserDataError<U>>
where U: JsLifetime<'js>, U::Changed<'static>: Any;

pub fn remove_userdata<U>(&self) -> StdResult<Option<Box<U>>, UserDataError<()>>
where U: JsLifetime<'js>, U::Changed<'static>: Any;

pub fn userdata<U>(&self) -> Option<UserDataGuard<U>>
where U: JsLifetime<'js>, U::Changed<'static>: Any;
```

Identical to 0.8.1. `UserDataGuard<'a, U>: Deref<Target = U>` is also
byte-identical (`rquickjs-core-0.12.2/src/runtime/userdata.rs:175-186`). Only
the `std` → `core`/`alloc`/`hashbrown` import plumbing changed.

**Non-obvious fact worth writing down:** userdata is stored on the **runtime**
opaque, not the context — `Ctx::get_opaque()` resolves via
`Opaque::from_runtime_ptr(JS_GetRuntime(ctx))`
(`rquickjs-core-0.12.2/src/context/ctx.rs:411-413`), and the map lives at
`rquickjs-core-0.12.2/src/runtime/opaque.rs:57`. It is cleared on runtime
teardown (`opaque.rs:291`). This matters for section 10: anything den puts in
userdata lives as long as the *runtime*, not the context.

---

## 9. Everything else den touches — verified unchanged

| API | Verdict | Source |
|---|---|---|
| `module::{Declarations, Exports}`, `Exports::export` | unchanged | `rquickjs-core-0.12.2/src/value/module.rs:128-181` |
| `Module::evaluate_def::<D, N>(ctx, name)` | unchanged | `value/module.rs:323-333` |
| `Module::declare(ctx, name, src)` | unchanged | `value/module.rs` |
| `ModuleDef` trait (both methods defaulted) | unchanged | `value/module.rs:113-123` |
| `BuiltinLoader`, `BuiltinResolver`, `FileResolver`, `ModuleLoader` (`with_module`, `with_path`, `with_pattern`) | unchanged, only the trait method gained `attributes` | `src/loader/{builtin_loader,builtin_resolver,file_resolver,module_loader}.rs` |
| `AsyncRuntime::{new, set_loader, set_interrupt_handler, set_max_stack_size, idle, drive}` | unchanged | `src/runtime/async.rs:108,185,198,232,313,365` |
| `AsyncContext::{full, with, async_with}` | `async_with` bound changed (§4), rest unchanged | `src/context/async.rs:160,231,216` |
| `Ctx::{clone, spawn, throw, catch, globals, run_gc, eval_with_options}` | unchanged | `src/context/ctx.rs:87,418,271,257,…` |
| `EvalOptions` (`global`/`strict`/`promise`/`backtrace_barrier`) | additive `filename: Option<String>` field; still `#[non_exhaustive]` + `Default` | `src/context/ctx.rs:28-41` |
| `Promise::into_future::<T>()` | unchanged | `src/value/promise.rs:152` |
| `Function::{new, call, call_arg, set_length, set_name, with_name}` | unchanged | `src/value/function.rs:47,62,75,104,122,128` |
| `function::Args::{new, push_args, apply}` | unchanged | `src/value/function/args.rs:30,81,128` |
| `prelude::{Opt, Rest, Func, This, Coerced, Async, …}` | byte-identical module | `src/lib.rs:76-95` vs 0.8.1 `src/lib.rs:68-84` |
| `ArrayBuffer::{new, new_copy, as_bytes, from_object}` | unchanged; additive `from_source*` + `ArrayBufferSource` | `src/value/array_buffer.rs:91,123,242,293,141-176` |
| `TypedArray::{new, new_copy, as_bytes, from_object, arraybuffer}` | unchanged; additive `U8Clamped`, `half` f16 | `src/value/typed_array.rs:128,137,208,195,219` |
| `Exception::throw_{message,syntax,type,reference,range,internal}` | unchanged | `src/value/exception.rs:105-211` |
| `Ctx::throw(value) -> Error` | unchanged | `src/context/ctx.rs:271` |
| `Array`, `Object`, `Coerced`, `BigInt`, `Value::{type_of,as_*,into_*}` | unchanged; additive `Object::new_proto`, `Value::new_big_int`, `is_big_int`, `is_proxy` | `src/value/{array,object}.rs`, `src/value.rs` |
| `Type` enum (`den-stdlib-console/src/lib.rs:62-190`) | **new `Proxy` variant, inserted before `Object`** — see §9a | `src/value.rs:540-558` |
| `convert::List` (`den-stdlib-networking/src/socket.rs:8`) | unchanged, still in `prelude` | `src/value/convert.rs`, `src/lib.rs` prelude |
| `String::from_str(ctx, s)` (`den-utils/src/serde_json.rs:20`) | unchanged | `src/value/string.rs` |
| `JsClass::constructor(ctx) -> Result<Option<Constructor<'js>>>` (`den-stdlib-text/src/lib.rs:163-166`, `den-stdlib-whatwg-fetch/src/lib.rs:179`) | unchanged | `src/class.rs:104` |
| `class::Trace` derive + `#[qjs(skip_trace)]` on foreign types | unchanged | `rquickjs-macro-0.12.2/src/trace.rs:148` |
| `prelude::*` glob (`den-stdlib-{text,sqlite,wasm}`) | module is byte-identical to 0.8.1, including the `#[cfg(feature = "multi-ctx")] MultiWith` gate (which existed in 0.8.1 too) | `rquickjs-core-0.12.2/src/lib.rs` vs `-0.8.1/src/lib.rs` |
| `Persistent::{save, restore}`, `FromJs`/`IntoJs` for `Persistent` | unchanged (only `std`→`core` imports) | `src/persistent.rs:88,102,114,125` |
| `IndexMap` `FromJs`/`IntoJs` (`indexmap` feature) | unchanged | `src/value/convert/{from,into}.rs:345-361 / 456-472` |
| `Either` `FromJs`/`IntoJs` (`either` feature) | unchanged | `src/value/convert/{from,into}.rs:109 / 148,164` |
| `Error` enum | additive `InvalidClass { class, message }`; the old `#[cfg(feature = "array-buffer")]` gates dropped | `src/result.rs` |
| `class::JsClass` | `const CALLABLE: bool` → `const KIND: ClassKind`; six defaulted `exotic_*` methods added | `src/class.rs` |

`JsClass::CALLABLE` → `KIND` only matters for hand-written `impl JsClass`.
den has none — every class goes through `#[rquickjs::class]`. den only *calls*
`JsClass::constructor`, whose signature is unchanged.

### 9a. Silent behaviour regression: `Type::Proxy` breaks `console.log(proxy)`

`rquickjs-core-0.12.2/src/value.rs:540-558` vs `-0.8.1/src/value.rs:511-528`:

```diff
     Exception: exception => JS_TAG_OBJECT,
+    Proxy: proxy => JS_TAG_OBJECT,
     Object: object => JS_TAG_OBJECT,
-    String: string => JS_TAG_STRING,
+    String: string => JS_TAG_STRING | JS_TAG_STRING_ROPE,
-    BigInt: big_int => JS_TAG_BIG_INT,
+    BigInt: big_int => JS_TAG_BIG_INT | JS_TAG_SHORT_BIG_INT,
```

Three consequences. **None of them is a compile error** — this is the one
change in the whole bump that will ship silently broken.

1. **`Value::type_of()` on a JS `Proxy` now returns `Type::Proxy`, not
   `Type::Object`.** `den-stdlib-console/src/lib.rs:62` matches `Type::Object`
   at `:135` and ends with a `_ => {}` catch-all at `:189`, so
   `console.log(new Proxy({}, {}))` now prints **nothing at all** where 0.8
   printed `{ … }`. Fix: widen the arm to `Type::Object | Type::Proxy =>`.
   The arm body needs no other change — the tag is still `JS_TAG_OBJECT`, so
   `Value::into_object()` and `Object::props()` still work on a proxy.
2. **Discriminants shifted.** `Proxy` is inserted *before* `Object`, so
   `Type::Object as u8` and every variant after it changed value. Nothing in
   den casts or serialises `Type` today; keep it that way.
3. `Type::interpretable_as(Object)` now also accepts `Proxy`
   (`rquickjs-core-0.12.2/src/value.rs:476`).

Rope strings and short bigints are internal QuickJS representations that 0.8
reported as `Type::Unknown`; folding them into `String`/`BigInt` is a fix, and
den's `Type::String` / `Type::BigInt` arms now simply fire more often.

---

## 10. WASM: holding JS values in a wasmtime/wasmi `Store` and calling back into JS

### 10.1 What den does today, and why it is on borrowed time

`den-stdlib-wasm/src/store.rs:7-18`:

```rust
pub type StoreData<'js> = (WasiP1Ctx, Ctx<'js>);

#[derive(Trace, Clone, From, Deref, DerefMut)]
#[rquickjs::class]
pub struct Store<'js> {
    #[qjs(skip_trace)]
    pub(crate) inner: Arc<RefCell<wasmtime::Store<StoreData<'js>>>>,
}

unsafe impl<'js> JsLifetime<'js> for Store<'js> {
    type Changed<'to> = Store<'to>;
}
```

`den-stdlib-wasm/src/lib.rs:148-153` then does:

```rust
#[qjs(evaluate)]
pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
    let engine = crate::engine::Engine::new();
    let store = crate::store::Store::new(&engine, ctx.clone());
    ctx.store_userdata(store)?;
```

and `den-stdlib-wasm/src/instance.rs:54-76` smuggles a `Persistent<Function>`
into the host closure:

```rust
#[derive(Clone, Copy, From, Deref, DerefMut)]
struct DangerouslyImplementSync<T>(T);
unsafe impl<T> Send for DangerouslyImplementSync<T> {}
unsafe impl<T> Sync for DangerouslyImplementSync<T> {}

let js_func: Persistent<Function> = v.get()?;
let js_func = Mutex::new(DangerouslyImplementSync(js_func));

let wasm_func = linker.func_new(module, name, ty, move |caller, params, results| {
    let (_, ctx) = caller.data();
    let func = js_func.lock().unwrap().0.clone().restore(ctx)?;
    …
});
```

Three problems, in ascending order of how much they will hurt:

1. **`JsLifetime` is being told a lie.** `Store<'js>` claims its `'js` is a
   JS-value lifetime; it is actually `Ctx<'js>`'s invariant context lifetime.
   The trait docs explicitly forbid this shape
   (`rquickjs-core-0.12.2/src/js_lifetime.rs:64-76`). It happens to be
   size/align-compatible so the `to_static` assertions in
   `runtime/userdata.rs:52-63` pass, but the guarantee is not there.
2. **wasmtime 48 requires `T: 'static`.** `Linker::func_new` is
   `pub fn func_new(…) -> Result<&mut Self> where T: 'static`
   (`wasmtime-48.0.0/src/runtime/linker.rs:408-417`), and the host closure must
   be `Fn(Caller<'_, T>, &[Val], &mut [Val]) -> Result<()> + Send + Sync + 'static`.
   `StoreData<'js>` is not `'static`. In wasmtime 27 this bound did not exist.
   This is a hard wall on the wasmtime bump, independent of rquickjs.

   It is not one error, it is ten. Verified `cargo check -p den-stdlib-wasm`
   output (see the Verification log) — every one of these disappears once
   `StoreData` is `'static`:

   ```
   error[E0477]: the type `(WasiP1Ctx, rquickjs::Ctx<'js>)` does not fulfill the required lifetime
     --> den-stdlib-wasm/src/store.rs:9:30, :13:23
   error: lifetime may not live long enough
     --> den-stdlib-wasm/src/store.rs:9:17, :30:9
     --> den-stdlib-wasm/src/global.rs:45:21
     --> den-stdlib-wasm/src/memory.rs:62:21, :73:21, :100:21
     --> den-stdlib-wasm/src/instance.rs:157:73
   error[E0521]: borrowed data escapes outside of associated function
     --> den-stdlib-wasm/src/instance.rs:219:13   (`import_object` escapes)
     --> den-stdlib-wasm/src/table.rs:88:19       (`ctx` escapes)
   ```
3. **Refcount cycle.** `Ctx::clone` is `JS_DupContext`
   (`rquickjs-core-0.12.2/src/context/ctx.rs:87-95`). The clone goes into
   userdata, which lives on the **runtime** (§8). So the `JSContext` cannot be
   freed until the runtime is torn down. Benign for den today (one context per
   runtime) but it is a real leak the moment den creates per-request contexts.

The `Mutex` in `instance.rs:60` is also dead weight: `Persistent<Function>` is
`Clone` (`persistent.rs:42-49`) and the closure is `Fn`, so `&self` access
suffices once the wrapper is `Sync`.

### 10.2 The facts you need

- `Ctx<'js>` is **`Send`** (`context/ctx.rs:103` `unsafe impl Send for Ctx<'_> {}`)
  but **not `Sync`**, is **refcounted** (`Clone` = `JS_DupContext`,
  `Drop` = `JS_FreeContext`, `ctx.rs:87-100`), and is **invariant** in `'js`
  (`_marker: Invariant<'js>`, `ctx.rs:84`). Invariance means
  `&Ctx<'static>` will *not* coerce to `&Ctx<'js>` — you cannot "narrow" a
  stored context by subtyping.
- `Ctx::from_raw(NonNull<qjs::JSContext>) -> Ctx<'js>` is **public and unsafe**
  (`ctx.rs:442-448`) and performs `JS_DupContext`, i.e. it *takes* a reference.
  `Ctx::as_raw() -> NonNull<qjs::JSContext>` is public and safe (`ctx.rs:512`).
  `qjs` is re-exported at `rquickjs-core-0.12.2/src/lib.rs:65`.
  Its safety contract: "user must ensure that a lock was acquired over the
  runtime and that invariant is a unique lifetime which can't be coerced to a
  lifetime outside the scope of the lock."
- `Persistent<T>` (`src/persistent.rs`) is the sanctioned way to hold a JS value
  past `'js`. `save(&ctx, v) -> Persistent<T::Changed<'static>>` records the
  **runtime** pointer; `restore(&ctx)` re-checks it and returns
  `Err(Error::UnrelatedRuntime)` on mismatch (`persistent.rs:88-111`). It is
  `Clone` when `T: Clone`. It is **neither `Send` nor `Sync`** (raw
  `*mut JSRuntime` field). There is no `Persistent` for a *context*.
- Consequence: **`Ctx<'js>` cannot be safely smuggled through a `'static` host
  closure.** No API in 0.12 does it for you, and there is no thread-local
  "current ctx". You must carry a context handle yourself, and at least one
  `unsafe` block is unavoidable.

### 10.3 The safe pattern

Confine the unsafety to **one** small type with a stated invariant, and keep
`StoreData` `'static` so wasmtime's bounds are satisfied without lying to
`JsLifetime`.

```rust
// den-stdlib-wasm/src/store.rs
use core::ptr::NonNull;
use std::{cell::RefCell, sync::Arc};

use derive_more::derive::{Deref, DerefMut, From};
use rquickjs::{class::Trace, qjs, Ctx, JsLifetime};
use wasmtime_wasi::p1::WasiP1Ctx;   // see §12 for the wasmtime-wasi 48 rename

/// Owning, lifetime-erased handle to the QuickJS context that created this store.
///
/// Why it exists: wasmtime requires `T: 'static` for `Store<T>` when registering
/// host functions (`Linker::func_new`, wasmtime-48.0.0 src/runtime/linker.rs:408),
/// so the `'js` lifetime cannot cross into the store. `Ctx` is refcounted, so
/// holding one keeps the `JSContext` alive; `with` re-materialises a real `'js`
/// only for the duration of a callback.
pub struct OwnedCtx(Ctx<'static>);

// SAFETY:
//  * `Ctx` is already `Send` (rquickjs-core-0.12.2 src/context/ctx.rs:103).
//  * `Sync` is asserted because every path that can reach this value runs under
//    the rquickjs runtime lock: a wasm host callback can only be entered from a
//    JS call, and `AsyncContext::{with, async_with}` hold `runtime.inner` for the
//    whole closure (src/context/async.rs:235, src/context/async/future.rs).
unsafe impl Send for OwnedCtx {}
unsafe impl Sync for OwnedCtx {}

impl OwnedCtx {
    pub fn new(ctx: &Ctx<'_>) -> Self {
        // SAFETY: `from_raw` performs `JS_DupContext`, so we own a reference and
        // the pointer cannot dangle for as long as `self` lives.
        Self(unsafe { Ctx::from_raw(ctx.as_raw()) })
    }

    /// Re-narrow the erased context to a callback-scoped `'js`.
    ///
    /// `Ctx` is invariant in `'js`, so a fresh `Ctx` has to be minted rather
    /// than reborrowed. The returned value never escapes `f`.
    pub fn with<R>(&self, f: impl FnOnce(&Ctx<'_>) -> R) -> R {
        // SAFETY: `self.0` holds a live reference to this context; the runtime
        // lock is held by whoever called into wasm (see the Sync note above).
        let ctx = unsafe { Ctx::from_raw(self.0.as_raw()) };
        f(&ctx)
    }
}

/// `'static` — this is what makes `wasmtime::Store<StoreData>` legal.
pub type StoreData = (WasiP1Ctx, OwnedCtx);

#[derive(Trace, JsLifetime, Clone, From, Deref, DerefMut)]
#[rquickjs::class]
pub struct Store {
    #[qjs(skip_trace)]
    pub(crate) inner: Arc<RefCell<wasmtime::Store<StoreData>>>,
}

#[rquickjs::methods]
impl Store {
    #[qjs(constructor)]
    pub fn new(engine: &crate::engine::Engine, ctx: Ctx<'_>) -> Self {
        let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new()
            .inherit_stdio()
            .inherit_env()
            .build_p1();
        Self {
            inner: Arc::new(RefCell::new(
                wasmtime::Store::new(&engine, (wasi_ctx, OwnedCtx::new(&ctx))),
            )),
        }
    }
}
```

What this buys:

- `Store` loses its `'js` parameter → **the manual `unsafe impl JsLifetime` in
  `store.rs:16-18` is deleted** and `#[derive(JsLifetime)]` covers it
  (lifetime-free types get `type Changed<'to> = Self`,
  `rquickjs-macro-0.12.2/src/js_lifetime.rs:145-151`).
- `ctx.userdata::<Store>()` no longer has an inferred `'js` to get wrong.
- `StoreData: 'static` satisfies wasmtime 48's `T: 'static`.
- Exactly two `unsafe` blocks, both with a written invariant.

The host closure then becomes:

```rust
// den-stdlib-wasm/src/instance.rs — inside resolve_imports
/// SAFETY: `Persistent` holds a `*mut JSRuntime`, which is why it is neither
/// Send nor Sync. Same argument as `OwnedCtx`: only reachable under the runtime lock.
struct SyncPersistent(Persistent<Function<'static>>);
unsafe impl Send for SyncPersistent {}
unsafe impl Sync for SyncPersistent {}

let js_func = SyncPersistent(v.get::<Persistent<Function>>()?);

linker.func_new(module, name, ty, move |caller, params, results| {
    let (_, owned_ctx) = caller.data();
    owned_ctx.with(|ctx| {
        let func = js_func.0.clone().restore(ctx)?;
        let mut args = Args::new(ctx.clone(), params.len());
        args.push_args(params.iter().map(|x| WasmValueConverter::from(*x)))?;
        let res: Value = func.call_arg(args)?;
        …
        Ok(())
    })
})?;
```

Note `caller.data()` returns `&StoreData`, so `owned_ctx` is `&OwnedCtx` — the
`Fn` (not `FnMut`) closure works without the `Mutex` that
`instance.rs:60` uses today.

### 10.4 Invariants you must not break

1. **Never let a `Ctx` minted by `OwnedCtx::with` escape the closure.** Keep
   `with` the only way to reach it; do not add an `fn ctx(&self) -> Ctx<'_>`.
2. **`Store` must be removed from userdata before the context dies** if den ever
   moves to multiple contexts per runtime. Userdata is runtime-scoped (§8), so
   `OwnedCtx` will otherwise pin a dead-in-spirit context until runtime teardown.
   Use `ctx.remove_userdata::<Store>()` (`context/ctx.rs:492`).
3. **Cross-runtime is already handled.** `Persistent::restore` validates the
   runtime pointer and returns `Error::UnrelatedRuntime`
   (`persistent.rs:106-109`) — do not add your own check.
4. **`RefCell` + reentrancy.** `Store` wraps `RefCell`. A JS host callback that
   re-enters wasm (JS → wasm → host → JS → wasm) will panic on the second
   `borrow_mut`. den's current `exports` getter
   (`den-stdlib-wasm/src/instance.rs:229-233`) already `borrow_mut`s inside a
   `Function::new` closure, so this is reachable today. Out of scope for the
   rquickjs bump, but note it before shipping the new host-callback feature.
5. **`ArrayBuffer` over wasm linear memory.** If you finish
   `Memory::buffer` (`den-stdlib-wasm/src/memory.rs:71-76`, currently
   `Err(ctx.throw("TODO".into_js(&ctx)?))` at `:75`), 0.12 adds safe
   constructors for externally-owned buffers:
   `ArrayBuffer::from_source` / `from_source_shared` / `from_source_immutable`
   plus the `ArrayBufferSource` trait
   (`rquickjs-core-0.12.2/src/value/array_buffer.rs:141-176`). That is a
   strictly better answer than the commented-out raw `JS_NewArrayBuffer` block.
   Caveat: wasm memory can be relocated by `memory.grow`, so a
   `from_source_shared` buffer must be detached (`ArrayBuffer::detach`,
   `array_buffer.rs:259`) around every `Memory::grow`.

---

## 11. Feature flags: `[features]` diff and what den should set

`rquickjs-0.8.1/Cargo.toml` vs `rquickjs-0.12.2/Cargo.toml`:

| Feature | 0.8.1 | 0.12.2 |
|---|---|---|
| `default` | `["classes", "properties"]` | `["std"]` |
| `full` | `chrono, loader, allocator, dyn-load, either, indexmap, classes, properties, array-buffer, macro, phf` | `std, chrono, loader, dyn-load, either, indexmap, macro, phf` |
| `allocator` | real | **deprecated no-op** |
| `classes` | real | **deprecated no-op** |
| `properties` | real | **deprecated no-op** |
| `array-buffer` | real | **deprecated no-op** |
| `rust-alloc` | `= ["allocator"]` | `= []` (standalone) |
| `parallel` | `[]` | `["std", "tokio/rt-multi-thread"]` |
| `std` | — | **new**, default |
| `bytes`, `half`, `disable-assertions`, `full-wasi`, `full-async-wasi` | — | **new** |
| `multi-ctx` | exists (gates `MultiWith`) | unchanged — **not** new, don't panic |

The four deprecated features are now empty and only exist to emit a
`#[deprecated]` note from `rquickjs-core-0.12.2/src/lib.rs:107-146`; their
functionality is unconditional. Counting gate sites: `allocator` and
`array-buffer` went from 14 `#[cfg]` sites each in 0.8.1 to 2 (the deprecation
shims) in 0.12.2 (`classes` 2→2, `properties` 3→2).

> **You will never see that deprecation warning.** The `#[deprecated]` items
> live inside `rquickjs-core`, so the warning is emitted while compiling a
> *dependency*, and cargo caps dependency warnings. `cargo check -p
> den-stdlib-wasm` today reports only two warnings, both unrelated. Treat the
> `array-buffer` removal below as hygiene you do by reading, not by chasing a
> diagnostic.

**`dump-leaks` still exists** in both (`rquickjs-0.12.2/Cargo.toml`,
`rquickjs-core-0.12.2/Cargo.toml`) — commit `87fb376` removed den's *own*
passthrough feature, not an upstream one. Nothing to restore.

Actions:

- Workspace `Cargo.toml` already reads
  `features = ["full-async", "rust-alloc", "parallel", "indexmap", "either"]`.
  That is correct for 0.12: `indexmap` and `either` are redundant (implied by
  `full`) but harmless, and `rust-alloc` is now the right way to ask for the
  Rust global allocator.
- `den-stdlib-wasm/Cargo.toml:20` requests
  `features = ["macro", "futures", "array-buffer"]` (and the same in
  `[dev-dependencies]`). **Drop `array-buffer`** — it is a deprecated no-op.

---

## 12. Not rquickjs — don't waste time debugging these

The same working tree bumped several other dependencies. This section was
re-verified against a fresh `cargo check` (see the Verification log); several
rows of the original list had **already been fixed in the tree** and are now
marked as such, so don't go looking for them.

### 12.0 Read this first: `den-transpiler-oxc` does not compile at all

This is the single biggest blocker and the original doc understated it as a
rename. `den-transpiler-oxc/Cargo.toml` carries the **oxc** dependencies but
`den-transpiler-oxc/src/lib.rs` is still the **swc** implementation:

```
den-transpiler-oxc/src/lib.rs:6-23: error[E0432]: unresolved import `sourcemap`,
  `swc_common`, `swc_config`, `swc_ecma_ast`, `swc_ecma_codegen`, `swc_ecma_parser`,
  `swc_ecma_transforms_base`, `swc_ecma_transforms_react`,
  `swc_ecma_transforms_typescript`, `swc_node_comments`
den-transpiler-oxc/src/lib.rs:56,72,73,94,162: error[E0433] (`sourcemap`,
  `swc_compiler_base`, `swc_ecma_transforms_typescript`, `anyhow`)
error: could not compile `den-transpiler-oxc` (lib) due to 18 previous errors
```

This is **not** gated behind `--features transpile`: `den-core` pulls the
crate in on the default feature set, so `cargo check -p den-core`,
`cargo check -p den` and `cargo build` all fail before rquickjs is even
reached. Port the transpiler to oxc (or temporarily revert the crate to swc)
before touching anything in §2 / §4, otherwise you cannot verify those edits.

`den-core/src/{engine.rs:12-18, loader/http.rs:7-11, loader/mmap_script.rs:7-11}`
still `use den_transpiler_swc::{…}` behind `#[cfg(feature = "transpile")]`;
those imports need renaming to `den_transpiler_oxc` as part of the same port.

### 12.1 Already fixed in the tree — ignore

| Symptom in the original list | Current state |
|---|---|
| `unresolved import wasmtime_wasi::preview1` | wasmtime-wasi 48 renamed the module to `p1`; there is no `preview1` in `wasmtime-wasi-48.0.0/src/`. Use `wasmtime_wasi::p1::WasiP1Ctx` (`store.rs:5`) and `wasmtime_wasi::p1::add_to_linker_sync` (`instance.rs:220`). **Check the tree before editing — this line has been flip-flopping.** |
| derive_more 1.0 → 2.1 `Deref`/`DerefMut` not in scope | fixed; `den-stdlib-core`, `den-stdlib-sqlite`, `den-stdlib-networking` all check clean |
| `unresolved import getset` (`den-stdlib-wasm/src/module.rs`) | fixed; no `getset` anywhere in den |
| `cannot find module or crate wabt` | fixed; `den-stdlib-wasm/src/lib.rs:103` now calls `wat::parse_str` |
| rand 0.8 → 0.9 (`den-stdlib-crypto`) | fixed; `den-stdlib-crypto` checks clean |

### 12.2 Still broken, still not rquickjs

`cargo check -p den-stdlib-wasm` currently reports **17** errors. Two are
rquickjs (§3). The rest:

| Symptom | Real cause |
|---|---|
| 10 × `E0477` / "lifetime may not live long enough" / `E0521` in `store.rs:9,13,30`, `global.rs:45`, `memory.rs:62,73,100`, `instance.rs:157,219`, `table.rs:88` | wasmtime 48's `T: 'static` on `Store<T>`. Fixed by §10.3. Listed in full in §10.1. |
| `error[E0004]: non-exhaustive patterns: ExternType::Tag(_) not covered` — `instance.rs:241`, `module.rs:85` | wasmtime 27 → 48 added the exceptions proposal's `Tag` extern |
| `error: cannot explicitly borrow within an implicitly-borrowing pattern` — `lib.rs:67,68` | Rust 2024 match-ergonomics tightening (`Either::Left(ref x)` inside a `&`-pattern) |

---

## 13. Suggested order of work

0. **Unblock the workspace: port `den-transpiler-oxc/src/lib.rs` off swc**
   (§12.0). Nothing downstream of `den-core` compiles until this is done, so
   you cannot verify steps 1-2 without it.
1. `den-core`: add the `attributes` parameter to the two `Loader::load` impls and
   the one `Resolver::resolve` impl (§2) — `loader/http.rs:24-25`,
   `loader/mmap_script.rs:39-40`, `resolver/http.rs:11-12`.
2. `den-core` + `src/`: swap the **four** `async_with!` invocations
   (`den-core/src/engine.rs:313,359`, `src/app.rs:51`, `src/main.rs:53`) for
   `.async_with(async |ctx| …)` and drop the three imports (§4).
3. `den-stdlib-console/src/lib.rs:135`: widen the arm to
   `Type::Object | Type::Proxy` (§9a). Silent regression otherwise.
4. `den-stdlib-wasm/Cargo.toml:19,28`: drop the `array-buffer` feature from
   both `[dependencies]` and `[dev-dependencies]` (§11).
5. Fix the **non-rquickjs** blockers in `den-stdlib-wasm` first (wasmtime 48
   `ExternType::Tag`, Rust 2024 `ref` patterns) — §12.2.
6. `den-stdlib-wasm`: rewrite `store.rs` per §10.3 (`OwnedCtx`, `'static`
   `StoreData`, delete the manual `JsLifetime`). This alone clears 10 of the
   17 errors. Then update the `ctx.userdata::<crate::store::Store>()` call
   sites — verified locations: `memory.rs:62,73,100`, `table.rs:88`,
   `instance.rs:217,231`, `global.rs:45`, plus the `Store::new` at
   `lib.rs:117`.
7. `den-stdlib-wasm/src/lib.rs:31-38`: wrap `ResultObject`'s two fields in
   `Class<'js, T>` and thread `'js` through `instantiate` (§3).
8. `den-stdlib-wasm/src/instance.rs:53-60`: drop the `Mutex`, use
   `OwnedCtx::with` in the `linker.func_new` closure (§10.3).
9. Optional cleanup: delete the now-redundant `#[qjs(declare)]` block at
   `den-stdlib-timer/src/lib.rs:77-84` (§5d). The other four `#[qjs(declare)]`
   blocks in den must stay.
10. Optional: honour `import … with { type: … }` in `HttpLoader` (§2d).

---

## Appendix A — actual compiler output

`cargo check` against the working tree (`rquickjs 0.12.2` already in
`Cargo.lock`), rustc stable:

**Clean (re-verified, see the Verification log):** `den-stdlib-text`,
`den-stdlib-console`, `den-stdlib-core`, `den-stdlib-crypto`, `den-stdlib-fs`,
`den-stdlib-io`, `den-stdlib-networking`, `den-stdlib-sqlite`,
`den-stdlib-timer`, `den-stdlib-whatwg-fetch`, `den-utils` — zero errors, zero
rquickjs-attributable warnings. Between them they exercise
`#[rquickjs::class]`, `#[rquickjs::methods(rename_all = "camelCase")]`,
`#[qjs(constructor)]`, `#[qjs(get, enumerable)]`, `#[qjs(get, enumerable, rename = …)]`,
`#[qjs(skip_trace)]`, `#[rquickjs::class(frozen)]`,
`#[rquickjs::module(rename/rename_vars/rename_types)]`, `#[qjs(evaluate)]`,
`JsClass::constructor(ctx)`, `Rest<Value>`, `Opt<…>`, `Either`, `IndexMap`,
`TypedArray::new_copy`, `ArrayBuffer::as_bytes`, `Exception::throw_*`,
`convert::List`, `Persistent`, `Type::*`, `prelude::*` — all still fine.

**`den-core`** — cannot currently be checked at all: it depends on
`den-transpiler-oxc`, which fails with 18 errors of its own (§12.0). The
rquickjs errors below are the ones it *will* produce once that is unblocked
(they were captured before the transpiler crate broke, and the trait
signatures in §2 were re-verified directly against
`rquickjs-core-0.12.2/src/loader.rs:64,98`):

```
error[E0050]: method `load` has 3 parameters but the declaration in trait `load` has 4
  --> den-core/src/loader/http.rs:25:18
   = note: `load` from trait: `fn(&mut Self, &Ctx<'js>, &str, Option<ImportAttributes<'js>>) -> Result<rquickjs::Module<'js>, rquickjs::Error>`

error[E0050]: method `load` has 3 parameters but the declaration in trait `load` has 4
  --> den-core/src/loader/mmap_script.rs:40:18

error[E0050]: method `resolve` has 4 parameters but the declaration in trait `rquickjs::loader::Resolver::resolve` has 5
  --> den-core/src/resolver/http.rs:12:16
   = note: `resolve` from trait: `fn(&mut Self, &Ctx<'js>, &str, &str, Option<ImportAttributes<'js>>) -> Result<std::string::String, rquickjs::Error>`

warning: use of deprecated macro `async_with`   -- den-core/src/engine.rs:5, :313, :359
```

**`den-stdlib-wasm`** (rquickjs-attributable only; **2 of 17** errors — the
original "2 of 30" predates the derive_more / rand / wasmtime-wasi fixes that
have since landed):

```
error[E0277]: using a `JsClass` type directly as a class field is not supported
  --> den-stdlib-wasm/src/lib.rs:33:5
   | `module::Module` implements `JsClass` — wrap the field in `Class<'js, T>` instead
   = note: nested mutations are lost because the generated getter clones the value
note: required by a bound in `rquickjs::class::impl_::JsClassFieldCheck::<T>::check`
  --> rquickjs-core-0.12.2/src/class/impl_.rs:112:12

error[E0277]: using a `JsClass` type directly as a class field is not supported
  --> den-stdlib-wasm/src/lib.rs:33:5
   | `instance::Instance` implements `JsClass` — wrap the field in `Class<'js, T>` instead
```

The other 15 are the dependency bumps catalogued in §12.2 — 10 wasmtime
`'static` lifetime errors, 2 `ExternType::Tag(_)` non-exhaustive matches, 2
Rust 2024 `ref`-pattern errors, and 1 rollup line.

---

## Verification log

Independent completeness/accuracy pass. Every claim below was checked by
reading the local crate source under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` or by running
`cargo check` in the den worktree.

### Claims re-verified as CORRECT (no change made)

| Claim | Evidence |
|---|---|
| `Resolver::resolve` / `Loader::load` gained `attributes: Option<ImportAttributes<'js>>` | `rquickjs-core-0.12.2/src/loader.rs:44,64-71` and `:96,98-102`, verbatim |
| `ImportAttributes<'js>(Object<'js>)`, `#[derive(Clone, Debug)]`, methods `get` / `get_type` / `keys() -> ObjectKeysIter<'js, String>` | `loader.rs:74-92` |
| Tuple impls fan `_attributes.clone()` out; `loader_impls!(A B C D E F G H)` | `loader.rs:258,291,308`. den's tuples at `den-core/src/engine.rs:44-222` (`set_loader` at `:223`) are within 8 |
| `JS_SetModuleLoaderFunc2` + `JS_SetModuleNormalizeFunc2` | `loader.rs:134-142` |
| `JsClassFieldCheck` / `NotAJsClassField` autoref-specialisation with the exact `#[diagnostic::on_unimplemented]` strings quoted in §3 | `rquickjs-core-0.12.2/src/class/impl_.rs:79-124`; injection at `rquickjs-macro-0.12.2/src/fields.rs:266-279` |
| `async_with!` carries `#[deprecated]` and forwards to `AsyncContext::async_with(&$context, async \|$ctx\| …)` | `rquickjs-core-0.12.2/src/context/async.rs:69-78` |
| `async_with` bound is now `for<'js> AsyncFnOnce(Ctx<'js>) -> R + ParallelSend` | `context/async.rs:216-222`; `WithFuture` still `Pin<Box<dyn Future>>` internally at `context/async/future.rs:36-58` |
| `Ctx::from_raw` is `pub unsafe`, does `JS_DupContext`; `Ctx::as_raw` is public+safe; `Ctx` is `Send`, invariant | `context/ctx.rs:442-450`, `:512-514`, `:103`, `:84` |
| `Ctx::from_raw_invariant` also exists (not in the doc, same contract) | `context/ctx.rs:431-434` |
| userdata trio unchanged; stored on the **runtime** opaque | `context/ctx.rs:480,492,503`, `:411-413` |
| `EvalOptions` gained `filename`, still `#[non_exhaustive]`, still has a `Default` impl | `context/ctx.rs:29-41`, `:67-78`. (Note: `filename` is itself `#[cfg(feature = "std")]`.) |
| `JsClass`: `const CALLABLE: bool` → `const KIND: ClassKind`, six defaulted `exotic_*` methods, new `ClassKind{Plain,Callable,Exotic}` | `rquickjs-core-0.12.2/src/class.rs:30-38,86-180` vs `-0.8.1/src/class.rs:30`. `Class::instance` still at `class.rs:223` |
| `Trace` additions: `Atom` :105, `Constructor` :111, `Proxy` :117, `TypedArray` :123; `Promise`/`ArrayBuffer` in the `base:` list | `class/trace.rs` |
| Prototype methods now installed with `Property::from(f).writable().configurable()` (non-enumerable) | `rquickjs-macro-0.12.2/src/methods/method.rs:309-315` |
| `ensure_no_conflicting_derives` rejects `FromJs`/`IntoJs` on `#[class]` | `rquickjs-macro-0.12.2/src/class.rs:351,499-529` |
| Module auto-declaration now keyed on the JS name | `rquickjs-macro-0.12.2/src/module/mod.rs:357-370`, `module.export(js_name, …)` writing into `self.declaration` at `:32-44` |
| `JsLifetime` trait byte-identical; blanket impl for `()` added at `:188`; `Ctx<'js>` still does **not** implement it | `rquickjs-core-0.12.2/src/js_lifetime.rs:79-82,188` |
| Feature table: `default` `["classes","properties"]`→`["std"]`; `full` contents; `allocator`/`classes`/`properties`/`array-buffer` are empty deprecation shims; `rust-alloc` no longer implies `allocator`; `parallel = ["std","tokio/rt-multi-thread"]`; `bytes`/`half`/`disable-assertions`/`full-wasi`/`full-async-wasi` new; `dump-leaks` still present | `rquickjs-{0.8.1,0.12.2}/Cargo.toml`, `rquickjs-core-{0.8.1,0.12.2}/Cargo.toml`, shims at `rquickjs-core-0.12.2/src/lib.rs:107-146` |
| `prelude` is byte-identical between versions | diffed `rquickjs-core-{0.8.1,0.12.2}/src/lib.rs` |
| §3 scope claim ("the only `#[qjs(get)]` **field** with a `JsClass` type") | `grep -rn '#\[qjs(get'` over den, 19 hits, all classified — see the new table in §3 |
| `Persistent<T>` holds a raw `*mut JSRuntime` (hence neither `Send` nor `Sync`) | `rquickjs-core-0.12.2/src/persistent.rs:37-40` |
| wasmtime-wasi 48 has no `preview1` module | `ls wasmtime-wasi-48.0.0/src/` → `p1.rs`, `p0.rs`, `p2/`, `p3/` |

### Claims found WRONG, and what was changed

1. **§3 line numbers.** `ResultObject` is at `den-stdlib-wasm/src/lib.rs:31-38`,
   not `:66-73`; it lives inside `#[rquickjs::module] pub mod wasm` (opened at
   `lib.rs:11`). Corrected, and the AFTER snippet now shows the import going
   into the *inner* module's `use rquickjs::{…}` at `lib.rs:17-20` — the file
   header has no rquickjs import at all, so following the old snippet would
   have produced an unresolved `Class`.
2. **§3 `Instance::new` reference.** It is at `instance.rs:212-216`, not `:215`;
   the current call site is `lib.rs:55`. Corrected.
3. **§3 / §10.4 / §13 `memory.rs` line numbers were fiction.** `memory.rs` is
   108 lines; the doc cited `:290`, `:299`, `:301`, `:328`, `:299-325`. Real
   locations: `#[qjs(get, enumerable)] buffer` at `:71-76` with the `TODO`
   throw at `:75`; `ctx.userdata::<Store>()` at `:62,73,100`. Likewise
   `table.rs:211` → `table.rs:88`, `instance.rs:262` → `instance.rs:229-233`,
   `Store::new` call at `lib.rs:117` (doc said `:148-153`). All corrected.
4. **§4 miscounts `async_with!`.** The doc says "three call sites"; §13 said
   "five". There are **four** invocations (`den-core/src/engine.rs:313,359`,
   `src/app.rs:51`, `src/main.rs:53`) plus three `use` imports
   (`engine.rs:5`, `app.rs:3`, `main.rs:6`). Replaced with a full table.
5. **§12 was largely stale.** Re-ran `cargo check`. `den-stdlib-core`,
   `den-stdlib-sqlite`, `den-stdlib-networking`, `den-stdlib-crypto`,
   `den-stdlib-fs`, `den-stdlib-io`, `den-stdlib-text`, `den-stdlib-console`,
   `den-stdlib-timer`, `den-stdlib-whatwg-fetch` and `den-utils` all compile
   **clean** — the derive_more `Deref`/`DerefMut`, rand 0.8→0.9, `getset` and
   `wabt` rows have all been fixed in the tree already. Split §12 into
   "already fixed — ignore" and "still broken".
6. **§12 understated the transpiler.** It is not "unresolved under
   `--features transpile`". `den-transpiler-oxc/src/lib.rs` is still the swc
   implementation while its `Cargo.toml` carries oxc, so it fails with 18
   errors on the **default** feature set and blocks `den-core`, `den`, and any
   verification of §2/§4. Promoted to §12.0 and to step 0 of §13.
7. **§12 / Appendix A error counts.** `den-stdlib-wasm` produces **17** errors,
   not 30; 2 are rquickjs. Corrected in both places.
8. **§10.1 point 2 was under-evidenced.** The wasmtime `T: 'static` problem is
   10 distinct errors across 6 files, not one. The verbatim list is now in
   §10.1 and summarised in §12.2.
9. **§11 implied a visible deprecation warning.** The `#[deprecated]` items are
   inside `rquickjs-core`, so cargo suppresses the warning as a dependency
   diagnostic — `cargo check -p den-stdlib-wasm` shows only two unrelated
   warnings. Added a callout so nobody hunts for a diagnostic that never
   appears.
10. **§5d cited `den-stdlib-timer/src/lib.rs:483-490`** in a 95-line file. Real
    location `:77-84`. Added a table classifying all five `#[qjs(declare)]`
    blocks in den by whether they are still needed.

### Gaps found and filled

- **New §9a — `Type::Proxy`.** `rquickjs-core-0.12.2/src/value.rs:540-558`
  inserts a `Proxy` variant into the `Type` enum *before* `Object`, so
  `Value::type_of()` on a JS proxy no longer returns `Type::Object`.
  `den-stdlib-console/src/lib.rs` matches `Type::Object` at `:135` and ends
  with `_ => {}` at `:189`, so `console.log(new Proxy({}, {}))` silently prints
  nothing. This compiles fine and the doc did not mention it at all — the only
  change in the whole bump that ships broken. Also documents the discriminant
  shift and the `JS_TAG_STRING_ROPE` / `JS_TAG_SHORT_BIG_INT` folding.
- **New §3a — the getter-*method* hazard.**
  `den-stdlib-networking/src/socket_addr.rs:36-38` returns `IpAddrWrapper` (a
  `JsClass`) by value from a `#[qjs(get)]` **method**. `JsClassFieldCheck` only
  fires on fields, so this is accepted, but it has the same clone semantics
  that issue #532 is about. Documented so the `ResultObject` fix is not
  "solved" by turning the fields into methods.
- **§9 table gaps.** Added rows for APIs den uses that the doc never listed:
  `Type` (`den-stdlib-console`), `convert::List`
  (`den-stdlib-networking/src/socket.rs:8`), `String::from_str`
  (`den-utils/src/serde_json.rs:20`), `JsClass::constructor`
  (`den-stdlib-text/src/lib.rs:163-166`,
  `den-stdlib-whatwg-fetch/src/lib.rs:179`), and the `prelude::*` glob imports.
- **§11 — `multi-ctx`.** Added a row noting it exists in *both* versions
  (gating `MultiWith`), so nobody mistakes it for a new feature they must
  enable. den does not use `MultiWith`.
- **§12.1 — the `preview1` → `p1` rename.** The doc named the error but never
  the replacement. Now states `wasmtime_wasi::p1::WasiP1Ctx` and
  `wasmtime_wasi::p1::add_to_linker_sync` explicitly, with a warning that this
  line has been flip-flopping in the working tree.
- **§13** gained the transpiler unblock as step 0 and the `Type::Proxy` fix as
  step 3, and every line reference in it was re-derived from the current tree.

### Commands run

```
cargo check -p den-core
cargo check -p den-stdlib-wasm
cargo check -p den-stdlib-core -p den-stdlib-sqlite -p den-stdlib-networking \
            -p den-stdlib-crypto -p den-stdlib-text -p den-stdlib-console \
            -p den-stdlib-timer -p den-stdlib-fs -p den-stdlib-whatwg-fetch \
            -p den-utils
grep -rn '#\[qjs(get'  --include='*.rs' .
grep -rn 'async_with'  --include='*.rs' .
grep -rn 'qjs(declare)' --include='*.rs' .
grep -rhoP 'rquickjs::[A-Za-z_:{][A-Za-z0-9_:]*' --include='*.rs' . | sort -u
diff rquickjs{,-core}-0.8.1/Cargo.toml rquickjs{,-core}-0.12.2/Cargo.toml
diff rquickjs-core-0.8.1/src/{lib,value,class,js_lifetime}.rs \
     rquickjs-core-0.12.2/src/{lib,value,class,js_lifetime}.rs
```

### Not verified

- `CHANGELOG.md` line references (`:47`, `:70`, `:85`, `:143`) and the upstream
  issue/PR numbers (#601, #532, #478, #429) were taken on trust; they are
  narrative, not load-bearing.
- The §10.3 `OwnedCtx` pattern is a design proposal and has not been compiled.
  Its two premises were verified (`Ctx::from_raw` is public+unsafe and dups the
  refcount; `Ctx` is `Send` and invariant), but the resulting `Store` rewrite
  has not been run against `cargo check`.
- den has no test suite exercising the JS-visible changes in §5b and §9a, so
  those behaviour claims rest on reading the rquickjs source, not on observed
  output.
