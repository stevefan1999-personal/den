# den:ffi — a plain-data FFI schema, and the case for not shipping it yet

Status: research, 2026-08-28. Snapshot of the den working tree (branch `master`, after `5fd9f82`),
rquickjs 0.12.2 (`full-async, rust-alloc, parallel, indexmap, either`), quickjs-ng via rquickjs-sys 0.12.2,
libffi-rs 5.2.0 / libffi-sys 4.2.1, dlopen2 0.8.2, tokio 1.53.1, TypeScript 5.9.3, Deno 2.9.4, Bun 1.3.9,
all on Linux x86_64. **Not a living document.** Every claim carries a `file:line` into the working tree or a
vendored source, or a line quoted verbatim from a probe run. Nothing is from memory. For the current truth read
[ARCHITECTURE.md](../../ARCHITECTURE.md) or the code.

Companion notes: [09](09-rquickjs-threads-and-event-loop.md) is the threading constraints doc this design is
built on; [14](14-runtime-feature-roadmap.md) has the four standing rules; [15 §3.18](15-stdlib-parity-gap.md)
is the requirements list; [15 §3.19](15-stdlib-parity-gap.md) is the permission gap this feature is gated on;
[16](16-cancellation-without-tokens.md) is the settled shutdown model.

## Sources read

| What | Path |
|---|---|
| den (working tree) | `den-stdlib-wasm/src/{instance,backend,store}.rs`, `den-stdlib-worker/src/port.rs`, `den-stdlib-sqlite/src/lib.rs`, `den-stdlib-fs/src/lib.rs`, `den-stdlib-core/src/exceptions.rs`, `den-core/src/engine.rs`, `src/main.rs`, `Cargo.toml`, `den-core/Cargo.toml`, `Cargo.lock`, `ARCHITECTURE.md` §7.5 |
| libffi-rs 5.2.0 | `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libffi-5.2.0/src/` (`libffi/` below) |
| libffi-sys 4.2.1 | `.../libffi-sys-4.2.1/{src/lib.rs,build/build.rs,build/not_msvc.rs,Cargo.toml}` |
| dlopen2 0.8.2 | `.../dlopen2-0.8.2/src/raw/common.rs` |
| rquickjs-core / quickjs-ng | `.../rquickjs-core-0.12.2/src/value/bigint.rs`, `.../rquickjs-sys-0.12.2/quickjs/quickjs.c` |
| Deno 2.9.4 type surface | `deno types` output saved at `/tmp/denplan/R8/deno.d.ts` |
| Bun 1.3.9 type surface | `bun-types@1.3.14` and `@1.4.0` `ffi.d.ts` under `/tmp/denplan/R8/` |
| Probes | `/tmp/denplan/{R7,R8,G-ffi,A-ffi,B-ffi}/` — table at the end |

---

## 0. TL;DR — the facts an implementer must not get wrong

0. **This feature has no P0 and no P1 row, and should probably not be built this cycle.**
   [15 §3.18](15-stdlib-parity-gap.md) line 1616 is 61 rows; every dlopen / typed-symbol / pointer /
   callback / N-API row in it is **P3**, and the section's own summary (line 1618) states the ordering:
   *"only after den:permissions, a feature-gated den:ffi shaped like Deno's table, skipping N-API, TinyCC and
   raw pointers."* den:permissions does not exist (§3.19 opener, line 1688: *"den has none of the permission
   surface"*), and is rated **P0**. The one P0 and all eight P1 rows in §3.18 belong to the **Rust embedding
   API and wasm**, not to a JS `den:ffi` module. The `#### den:ffi / embedding API (6)` list at
   [15](15-stdlib-parity-gap.md):3798 is, verbatim by status: *Embed the runtime as a library from a host
   program* [present]; *Register downstream Rust modules/ops into the engine (extension registry)* [partial];
   *Expose Rust structs as JS classes with methods/getters via macros* [den_better]; *Promise-returning native
   functions from Rust async fns* [present]; *Host-supplied module resolver/loader composed into the chain*
   [partial]; *Embedder can restrict which module sources the engine trusts* [missing]. So it is **three items
   of work, not four** — the two `partial`s (`EngineBuilder::with_module`, `prepend_resolver`/`prepend_loader`)
   and the one `missing` (loader-chain-as-data). The wasmtime `epoch_interruption` row is **not** in that list:
   it is at 15:3759 under `#### den:wasm (15)`, *"Interrupt a running WebAssembly computation from the host"*
   [missing]. den:http is P0 with three P0 rows. **Recommendation: shelve this design, build `EngineBuilder`
   plus the wasm epoch row, and keep WebAssembly as den's sanctioned portable-native path.** The rest of this
   note is what to build *when its turn comes*, and it is complete enough to start from.

1. **libffi guarantees exactly one thing: it turns a signature known at runtime into a correct call.**
   `middle::Cif::new(args, result)` builds an `ffi_cif` and `Cif::call`/`call_return_into` performs the ABI's
   argument classification, register/stack placement, struct-by-value class (SysV register vs MEMORY), and
   hidden `sret` pointer. It guarantees **nothing** about whether the signature you gave it matches the C
   function. A wrong signature is undefined behaviour with no diagnostic available at any layer.

2. **`libffi::high` cannot express a runtime symbol table; only `middle` can.** `high` is fixed-arity
   `Closure0..Closure12` (`libffi/high/mod.rs:1-24`) with `unsafe trait CType: Copy` implemented only for
   primitives and raw pointers (`high/types.rs:43`, impls at `:74-79`, `:130`, `:137`) and a compile-time
   return type. `low` is raw bindgen that re-implements `middle`'s memory management. So: `middle` only.

3. **`call_return_into` writes exactly `type.size()` bytes; it is `call<R>` that widens — and the design never
   uses `call<R>`.** `low::call<R>` (`libffi-5.2.0/src/low.rs:415`) comments *"libffi always writes at least a
   full register to the result pointer"* and branches at `:423` on `size_of::<R>() < size_of::<usize>()`,
   bouncing through a `MaybeUninit<usize>`. `low::call_return_into` (`:549`) does the **same correction on the
   other side**: it reads `return_type_size` from the CIF and at `:565` passes `ret` straight through only when
   `return_type_size >= size_of::<usize>()` or the type is FLOAT / STRUCT / VOID; otherwise it calls into the
   register-sized temporary itself and finishes with `ptr::copy_nonoverlapping(src_ptr, ret.cast(),
   return_type_size)` at `:604` — exactly `type.size()` bytes, endian-corrected. So an **exactly-sized cell is
   correct**, and that is precisely why a dynamic marshaller can use `call_return_into` at all. Probed with a
   canary struct `{ cell: [u8;8], canary: [u8;8] }` prefilled `0xAA` and `Ret::new(&mut g.cell[0])`
   (`/tmp/denplan/V2/rust/src/main.rs`):

   ```
   P2 ret_i8  size=1 bytes-modified=[0]          cell=[ff,aa,aa,aa,aa,aa,aa,aa] canary=[aa,...]
   P2 ret_i16 size=2 bytes-modified=[0, 1]       cell=[fe,ff,aa,...]            canary=[aa,...]
   P2 ret_i32 size=4 bytes-modified=[0, 1, 2, 3] cell=[fd,ff,ff,ff,aa,...]      canary=[aa,...]
   ```

   No byte past `size` is touched. **The real trap is a wrong `R` on `Cif::call<R>`** — that one *does* write a
   full register and is an out-of-bounds write on a small `R` — which is one more reason the design uses
   `call_return_into` exclusively (§7).

4. **`Cif` and `Closure` are `!Send + !Sync`; `libloading::Library`/`dlopen2` are `Send + Sync`.**
   Compile probe (R7, `src/bin/negative.rs`, not retained): ``error[E0277]: `*mut *mut ffi_type` cannot be sent
   between threads safely … within `Cif` … required because it appears within the type `Closure<'static>` ``,
   tracing through `libffi-sys-4.2.1/src/lib.rs:176` → `libffi/middle/mod.rs:130` → `:422`. `grep -rn
   'unsafe impl' libffi-5.2.0/src/` returns only the three `CType` impls. **Consequence:** a `Cif` cannot be
   captured into an `async_with` closure (which is `Send` under `parallel`) and cannot cross to
   `spawn_blocking` — it must be rebuilt on the far side from plain `Send` data.

5. **`!Send` on the closure is not protection. C calls the trampoline from whatever thread it likes.**
   R7 probe p4, against a C function that `pthread_create`s: `P4   [callback] running on ThreadId(2)
   (main was ThreadId(1)) -- SAME? false` / `P4 apply_cb_on_thread -> 42  (C spawned a pthread and called the
   SAME code ptr)`. Callbacks are the hard half of FFI, and this is why.

6. **A synchronous FFI call whose C function invokes a callback from a thread it spawned DEADLOCKS den.**
   The realm thread is parked inside the C frame, so `idle()` is not running, so the `ctx.spawn`-ed pump that
   is the only legal way to reach JS cannot run, so the foreign thread blocks for ever. R7 seam case b:
   `b JS is about to call the C function (this frame holds the runtime lock)` /
   `[cb on ThreadId(4)] FOREIGN thread -> posted to realm, blocking` / then nothing / `timeout 12 ./seam b`
   → `[exit=124]`. Cases a, c and d all exit 0. **den refuses the shape rather than detecting it:** passing a
   `Callback` to a symbol *not* declared `nonblocking` is a typed error at marshal time (§4.7).
   **The refusal does not close the whole hole.** A callback registered through one (`nonblocking`) symbol,
   stored by C, and later fired by a *different* sync symbol that joins the firing thread reaches this same
   deadlock without ever passing a `Callback` to the sync symbol — so there is nothing at that call site to
   refuse. Reproduced at `/tmp/denplan/V2/rust/src/bin/residual.rs`, `[exit=124]`. §4.7 carries the mitigation
   (a bounded `blocking_recv`) and §5.1 carries what remains.

7. **A Rust panic — or an uncaught JS exception surfaced as one — inside the trampoline aborts the process.**
   `extern "C"` cannot unwind. R7 `src/bin/panic_cb.rs`: `panic in a function that cannot unwind` … frame 20
   `ffi_closure_unix64_inner at .../src/x86/ffi64.c:963:3` … `thread caused non-unwinding panic. aborting.`
   `[exit=134]`. libffi's own `high` layer concedes the same by wrapping every callback in `abort_on_panic!`
   (`high/mod.rs:85-106`). Every trampoline body goes in `catch_unwind`; a JS throw becomes the zero C value
   plus `den_stdlib_core::exceptions::report_exception` (`den-stdlib-core/src/exceptions.rs:46`).

8. **THE BIGINT RANGE TRAP — [doc 09 fact 12](09-rquickjs-threads-and-event-loop.md), and it is worse than
   doc 09 records.** `BigInt::to_i64()` silently corrupts out-of-range values in two different ways:
   `2n**64n - 1n → Ok(-1)` (wrap) and `2n**70n → Ok(0)` (zero), reproduced in R7 `seam/src/bin/marshal.rs`
   (`M1 2n**64n - 1n   to_i64=Ok(-1)`, `M1 2n**70n        to_i64=Ok(0)`) and independently by doc 09's probe
   T10. Doc 09's remedy — *"stringify anything outside i64"* — is the right one, because **the obvious guard
   is itself broken**: quickjs-ng's `BigInt.asUintN` is spec-violating for every bit width that is a multiple
   of 32. `M1b BigInt.asUintN(64, 18446744073709551615n).toString() => -1`;
   `M1c BigInt.asUintN(32, 4294967295n) => -1`; while `asUintN(8,255n)`, `asUintN(16,65535n)`,
   `asUintN(63, 2n**63n-1n)` and `asUintN(65, 2n**64n-1n)` are all correct. Root cause
   `rquickjs-sys-0.12.2/quickjs/quickjs.c:57235-57296` `js_bigint_asUintN`: `shift = (-bits) &
   (JS_LIMB_BITS - 1)` is `0` when `bits % 32 == 0` (`JS_LIMB_BITS 32` at `quickjs.c:442`), so the sign bit is
   never cleared. **This is not an FFI bug, and there is nothing in den to fix.**
   `grep -rn 'asUintN' --exclude-dir=target --exclude-dir=.git --exclude-dir=.qwen .` over the working tree
   returns hits only inside this document and inside `vendor/test262/test/built-ins/BigInt/asUintN/` — zero in
   den's own Rust or JS. So the remediation is a **single upstream quickjs-ng report**, not a repo sweep. What
   it means for den:ffi is simply that `asUintN` may not be used as the u64 guard (§4.4, §7).

9. **Zero new crates for library loading. `dlopen2` 0.8.2 is already in den's default build graph.**
   `cargo tree -i dlopen2 -e normal` → `dlopen2 v0.8.2 └── rquickjs-core v0.12.2 └── rquickjs v0.12.2 ├── den
   v0.4.0`; `Cargo.lock:1404`. It gives `raw::Library::open_with_flags` (`dlopen2/src/raw/common.rs:100`),
   `symbol` (`:154`), and `Drop == dlclose` (`:191`), plus explicit RTLD flag control that neither Deno nor Bun
   exposes. `grep -c 'name = "libffi"' Cargo.lock` → `0`: libffi is the **only** new crate.

10. **`as_bytes()` is honest about byteOffset and detects detachment — and detects nothing else, and is
    read-only.** R7 marshal: `M3 Uint8Array as_bytes ptr=0x55cc6dfd4f68 len=4 data=[1, 2, 3, 4]`; a
    `new Uint8Array(buf, 2)` view gives `ptr` two bytes past the base; a detached buffer gives
    `as_bytes = Ok(None)` — not UB, not a panic. Two limits an earlier draft glossed over:
    (a) **`as_bytes` yields `&[u8]`** (`rquickjs-core-0.12.2/src/value/typed_array.rs:208-211`), so it cannot be
    the handle for a C function that *writes* into the buffer — that needs
    `TypedArray::as_raw() -> Option<RawArrayBuffer>` (`typed_array.rs:213`,
    `RawArrayBuffer { len: usize, ptr: NonNull<u8> }` at `array_buffer.rs:57-60`).
    (b) **detachment is its only failure signal.** `get_raw_bytes` (`typed_array.rs:253-275`) returns `None`
    for a detached buffer and nothing else, and `ArrayBuffer`'s whole public surface
    (`array_buffer.rs:230-304`: `len`, `is_empty`, `as_bytes`, `as_slice`, `detach`, `as_value`, `as_raw`)
    exposes no `is_shared`, no `resizable` and no `maxByteLength`. Both hazards are live in den *today* —
    `den bufs.js` prints `SharedArrayBuffer: function` and
    `new ArrayBuffer(8, { maxByteLength: 16 }).resizable === true` — so refusing them takes an explicit
    mechanism (§4.6), not a side effect of `as_bytes`.
    The pointer is valid for exactly one synchronous `Cif::call` and must never be stored. Bytes handed *to* JS go
    through `TypedArray::new_copy`; `ArrayBuffer::new` with a `Vec` is the double-free of doc 09 fact 12.

11. **`Box::leak` of anything holding a parked `Ctx` aborts the process at shutdown.** R7's first seam run
    leaked its `Slot`+`Closure`: `seam: …/out/quickjs.c:2348: JS_FreeRuntime: Assertion
    'list_empty(&rt->gc_obj_list)' failed.` / `[exit=134]` + core dump. Moving both into a
    `#[derive(JsLifetime)]` context-userdata struct made every case exit 0. The assertion names nothing about
    FFI, which is what makes it expensive to diagnose.

12. **`close()` cannot be made safe from C's side, only from JS's.** R7 p6 registers a closure with C, drops
    it, then asks C to fire it: `P6 registered; live call = 7` / `P6 closure dropped, C still holds the
    pointer; firing it now` → `[exit=139]` (SIGSEGV). Same for a stale symbol address after `Library::close`
    (`src/bin/unload.rs`, `[exit=139]`). A liveness flag turns *"JS touches it after dispose"* into a typed
    throw; nothing turns *"C touches it after dispose"* into anything.

---

## 1. Teaching — what FFI actually is, and why callbacks are the hard half

### 1.1 The ABI, not the header

A shared library exports addresses. `libprobe.so` contains, at some offset, the machine code for `add_i32`;
`dlsym` hands back its address and **nothing else**. There is no type information in an ELF/Mach-O/PE symbol
table beyond the name — no arity, no widths, no signedness. Everything a caller needs to place arguments
correctly comes from a *convention* the compiler on both sides agreed to, and the caller has to reconstruct it.

That convention is not "push the arguments". On x86-64 SysV, integer and pointer arguments go in
`rdi, rsi, rdx, rcx, r8, r9` then the stack; floats go in `xmm0..xmm7` with a **separate** counter; `al` must
hold the number of vector registers used when calling a variadic function; a struct is classified field-group
by field-group into INTEGER / SSE / MEMORY classes, and a MEMORY-class struct is passed by a hidden pointer in
`rdi` that shifts every other argument down one register. AArch64 has a different rule set again (HFA/HVA:
a struct of up to four same-typed floats goes in `v0..v3`). Windows x64 passes any struct larger than 8 bytes
by reference. Getting this wrong does not raise an error; it reads the wrong register and returns a plausible
number.

This is the entire argument for libffi, and it is the only one that matters. It is also exactly the part Bun
refused to implement: passing a struct type to `bun:ffi` throws
`Error: param must be a string (type name) or number` (R8 `bun_probe.ts`).

### 1.2 Why the CIF has to be built at runtime

A normal Rust `extern "C"` declaration is a compile-time signature: rustc emits the call sequence. den cannot
do that — the signature arrives as a JS object at run time. libffi's answer is the **CIF** (Call InterFace):
`ffi_prep_cif` takes an array of `ffi_type*` plus a return `ffi_type*` and precomputes the placement plan;
`ffi_call` then walks an array of `void*` argument pointers and executes it. In libffi-rs that is
`middle::Cif::new(args: impl IntoIterator<Item = Type>, result: Type)` and
`Cif::call_return_into(fun, args, ret)`.

Verified end to end in R7 p1 (signature parsed from a runtime `["i32","i32"]`-shaped string list):

```
P1 add_i32(["i32", "i32"]) -> i32 = 42
P1 add_u64(["u64", "u64"]) -> u64 = 18446744073709551615
P1 add_f64 -> f64 = 3.75
P1 my_strlen(["ptr"]) -> u64 = 10
P1 noop_void([]) -> void = <void>
```

and for structs, both ABI classes, in R7 p2:

```
P2 point_scale(Point { x: 3, y: -4 }, 5) -> Point { x: 15, y: -20 }  (by value in AND out)
P2 triple_sum(Triple { a: 1.0, b: 2.0, c: 3.0 }, 0.5) -> Triple { a: 1.5, b: 2.5, c: 3.5 }  (24-byte MEMORY-class struct)
```

The 24-byte case is the interesting one: it is MEMORY class, so the real C call has a hidden `sret` pointer as
argument zero. libffi hides that completely — den passes two arguments and gets a value back.

### 1.3 Callbacks are the hard half

Calling *out* is a one-way trip: marshal, call, marshal back, done. Calling *in* inverts every constraint.

C wants a **function pointer**. A JS `Function` is not one, and cannot become one — it is a GC-managed object
in a `JSContext`. libffi's `Closure` bridges this by allocating an executable trampoline whose code pointer C
can hold, and which calls a Rust `extern "C" fn(cif, ret, args, userdata)`. R7 p3:
`P3 C called back into Rust: apply_cb(closure, 6, 7) = 42, closure ran 1 time(s)`.

Three things now go wrong at once:

1. **The userdata cannot hold a JS value.** The trampoline is `extern "C"` and `'static`; `Function<'js>` is
   neither `Send` nor `'static` (doc 09 fact list, `Value/Object/Function/Promise` are `!Send`).
2. **The thread is not ours.** Fact 5 above: C called the code pointer from a pthread it created. Every JS
   value in the process is off-limits on that thread.
3. **We may not be able to get back in.** Even on our own thread, "run JS now" is not free: `idle()` holds the
   runtime mutex for its entire parked lifetime (doc 09 fact 4, probe T12 `with() blocked while idle()
   pending=true`), so the *only* way to run JS in a context while `idle()` runs is a `ctx.spawn`-ed future
   (doc 09 fact 3/4).

### 1.4 den has already solved this once — for wasm

Every constraint in §1.3 is the constraint `den-stdlib-wasm` hit, and the pattern it chose is directly
reusable. The doc comment states the problem in exactly these terms
(`den-stdlib-wasm/src/instance.rs:34-37`):

> A wasm host callback must be `Send + Sync + 'static`, which no JS value is. The callback therefore captures
> an index into this registry and reaches the function through the `Ctx` parked in the store payload.

The three moving parts:

| Piece | wasm | den:ffi |
|---|---|---|
| JS function registry in context userdata | `ImportedFunctions<'js> { functions: RefCell<Vec<Function<'js>>> }`, `instance.rs:40`; `register` returns `functions.len() - 1` at `instance.rs:45-53` | identical, in `FfiRealm<'js>` |
| Lifetime-free thing the `'static` closure captures | `struct HostFunction { index: usize, signature: FuncType }`, `instance.rs:95`, commented *"Lifetime-free on purpose: this is what the engine's host callback closure captures, and that closure has to be `'static`"* | `struct Slot { index, owner: ThreadId, live, mailbox, reentrant }` |
| Re-entry into JS | `caller.data().with_ctx(...)` → `OwnedCtx::with`, `backend.rs:151` → `:111` | same `OwnedCtx`, same-thread branch only |

`OwnedCtx` (`den-stdlib-wasm/src/backend.rs:98`) is a `Ctx<'static>` obtained by `Ctx::from_raw(ctx.as_raw())`
(a `JS_DupContext`), with `with<R>(&self, f: impl FnOnce(&Ctx<'_>) -> R)` at `:111` minting a callback-scoped
`'js` on demand because `Ctx` is invariant. Its SAFETY comment (`backend.rs:112-119`) states the precondition
den:ffi needs verbatim: *"a host callback is only entered from a JS call that holds the lock for the whole
closure"* — which is true for the same-thread branch and false for the foreign-thread branch. That is the whole
seam. (`with_ctx`, the wasm-side wrapper, is at `:151`.)

**If** den:ffi is built, `OwnedCtx` should move to `den-util` and be re-exported from `den-stdlib-wasm` — one
25-line type with the most delicate unsafe block in the tree, two users, one copy. That move is scheduled in
**phase 4** and belongs nowhere earlier. Fact 0's recommendation is not to build den:ffi this cycle, which makes
`OwnedCtx` a **one-user** type until phase 4 actually happens; performing the move on the strength of this
sentence alone is churn for no second user. Two further caveats, from §4.7 below: the second user is a branch
that turns out to be nearly unreachable under the design's own rules, and phase 0 of [doc 18](18-den-http.md)
must not "fix" `OwnedCtx` while sweeping for cloned-`Ctx` hazards — it is safe precisely because it lives in
userdata, which `rquickjs-core-0.12.2/src/runtime/opaque.rs:284-292` clears before `JS_FreeRuntime`.

Verified working against a real rquickjs realm in R7's seam probe, same-thread case c:

```
c JS is about to call the C function (this frame holds the runtime lock)
[cb on ThreadId(1)] SAME thread as the realm -> re-entrant direct call
c JS got 307
```

---

## 2. Competitor surfaces

Both probed on this machine against the same C shim (`/tmp/denplan/R8/probe.c`, `cc -shared -fPIC`).

### 2.1 Deno 2.9.4

`Deno.dlopen(path, symbols)` behind `--allow-ffi[=PATH]`. There is no `--unstable-ffi` in 2.9.4, though the
help still labels the permission "(Unstable)". Denial is typed:

```
error: Uncaught (in promise) NotCapable: Requires ffi access to "/tmp/denplan/R8/libprobe.so",
run again with the --allow-ffi flag
```

Symbol shape (`/tmp/denplan/R8/deno.d.ts:6398-6431`): `{ name?, parameters, result, nonblocking?, optional? }`
plus `{ type }` statics. Types (`:6150-6203`): 8 plain-number types, 4 BigInt types (`u64 i64 usize isize`),
`bool`, `pointer`, `buffer`, `function`, and `{ struct: readonly NativeType[] }`. Pointers are opaque
null-prototype objects (`:6494`), read through a length-less `UnsafePointerView` (`:6536`).

Full probe passes: `add_i32(35,34) = 69`, nonblocking `= 69`, `point_translate -> Uint8Array(8) [15,0,0,0,
25,0,0,0]`, `triple_sum -> 7`, `call_back_n(cb,4) = 60`, cstring in and out, `fill_bytes`,
`definitely_missing (optional) = null`.

Four defects worth naming:

- **Struct arguments are not bounds-checked.** `point_translate(new Uint8Array(4), 1)` for a declared 8-byte
  struct: `undersized -> Uint8Array(8) [1,0,0,0, 1,0,0,0]`, no throw. That is an out-of-bounds read reachable
  from pure JS with only `--allow-ffi`. Oversized is likewise accepted.
- **No layout helper.** `{ struct: NativeType[] }` is positional, so the caller hand-packs ABI padding with no
  way to ask what den— or rather Deno — expects. `{struct:["i8","i32"]}` (C size 8) is accepted silently.
- **`threadSafe()` is misnamed and its absence fails silently.** The doc (`:6666-6672`) concedes all callbacks
  are already thread-safe; `threadSafe()` only refcounts the loop and wakes it. Without it a foreign-thread
  call is **dropped** and the process exits 0: `[plain] main script done; nothing else keeps the loop alive` →
  `exit 0`, callback never printed. With it: `>> [threadsafe] cb ran with 4242`.
- **`close()` has no liveness guard.** Closing from inside a live foreign-thread callback → `exit 139`
  (SIGSEGV). Deferring to a later turn (`setTimeout(() => cb.unref(), 400)`) → exit 0.

Type inference is the best part: `dlopen<const S extends ForeignLibraryInterface>` (`deno.d.ts:6821`) — the
TS 5.0 `const` type-parameter modifier means **no `as const` is needed** even though the JSDoc examples still
show it. `deno check /tmp/denplan/R8/infer_deno.ts` (zero `as const`, three `@ts-expect-error`) prints only
`Check infer_deno.ts`.

### 2.2 bun:ffi 1.3.9

No permission gate at all — every Bun probe below ran with no flags and dlopen'd an arbitrary `.so`. Even
`--no-addons`, which blocks `process.dlopen`, does not block `bun:ffi` ([15 §3.18](15-stdlib-parity-gap.md)
row, line 1633).

- **No structs.** `Error: param must be a string (type name) or number`. The `FFIType` enum has no struct
  member.
- **Pointers are JS numbers.** `type Pointer = number & { __pointer__: null }` (ffi.d.ts:346). Probe:
  `greeting_ptr() = 140220811587584 typeof = number`. Forgeable, arithmetic-able, and lossy above 2^53 —
  `2**53+1` is `9007199254740992`.
- **`returns: "cstring"` gives a String *object*.** `class CString extends String` (1.3.14 ffi.d.ts:1031).
  `greeting === 'hello from C'` is **false**; `String(greeting) === 'hello from C'` is true. Equality,
  `switch`, and `Map` keys all misbehave. Worse, bun-types@1.4.0 declares `type CString = string` with a
  callable constructor, while bun 1.3.9 throws `TypeError: Cannot call a class constructor CString without
  |new|` — the shipped types do not match the engine.
- **`nonblocking` is silently accepted and ignored.** Every `bun:ffi` call blocks the JS thread. A user porting
  Deno code gets a behavioural change with no error.
- **A missing `threadsafe: true` crashes the process with a lie.** Same C pthread shim:
  `RangeError: Maximum call stack size exceeded.` / `Bun v1.3.9 (Linux x64)` and the process dies. With
  `threadsafe: true`: `>> THREADSAFE cb ran with 4242`. The unsafe mode is the default.
- **Callbacks are untyped.** `constructor(callback: (...args: any[]) => any, definition: FFIFunction)`
  (ffi.d.ts:1070). `new JSCallback((anything: string) => ({ not: "a number" }), { args: ["i32"], returns:
  "i32" })` typechecks clean under tsc 6.0.3.
- **`as const` is mandatory.** `dlopen<Fns extends Record<string, FFIFunction>>` (ffi.d.ts:586) has no `const`
  modifier and `args` is `readonly FFITypeOrString[]`, not a tuple. `tsc /tmp/denplan/R8/tsprobe/infer_bun2.ts`
  reports errors **only** on the `as const` lines: `infer_bun2.ts(5,16): error TS2554: Expected 2 arguments,
  but got 1.` and `infer_bun2.ts(7,30): error TS2554: Expected 2 arguments, but got 4.` — lines 4 and 6, the
  same calls without `as const`, produce nothing.
- **`cc()` genuinely works.** TinyCC in-process: `cc() mul(6,7) = 42`, `cc() who() = tinycc`. A real capability
  neither Deno nor den has, and a whole C toolchain of attack surface.

### 2.3 Score

| | Deno | Bun | den (planned) |
|---|---|---|---|
| Permission gate | path-scoped, typed | none | cargo feature OFF + grant value |
| Structs by value | positional, unchecked | none | named fields, layout cross-checked |
| Pointer | opaque object, no provenance | JS number | opaque, provenance-carrying |
| Pointer view bounds | none (no length) | none | **length mandatory, but not bounds-checked** (§4.5, §5.1) |
| 64-bit | BigInt | BigInt + lossy `*_fast` | BigInt only |
| Off-thread call | opt-in `nonblocking` | accepted and **ignored** | opt-in `nonblocking`, honoured (§4.3) |
| Foreign-thread callback failure | silent drop | process crash | typed refusal at marshal time; bounded stall otherwise |
| `as const` needed | no | **yes** | no |
| Callback typed | handle untyped | no | typed handle, via a signature brand (§3.2) |
| in-process C compiler | no | yes | refused |

---

## 3. The schema

### 3.1 Plain data, not a builder

A Zod/ArkType-style DSL has one genuinely transferable idea and one trap.

The idea: **a schema is a value that serves three masters at once** — Rust walks it to build the CIF and cache
struct offsets, TypeScript reads it to type the call site, and the boundary validates against it. Both Zod
versions do the inference the same way, through a phantom slot plus one indexed access (`zod@3` `types.d.ts:17`
`export type TypeOf<T extends ZodType<any,any,any>> = T["_output"]`; `zod@4` `core.d.ts:55`
`T extends { _zod: { output: any } } ? T["_zod"]["output"] : unknown`). Nothing about that mechanism requires
the schema to be a class or a builder function.

The trap: a builder (`fn([i32, i32], i32)`, `struct({x: f64})`) means den:ffi exports a **type vocabulary** —
a dozen runtime values whose only job is to be arguments to other runtime values. Plain data exports one
function. `"i32"` is a string literal that exists only in the `.d.ts`, so there is nothing to register, nothing
to keep in sync, and nothing to serialize wrongly. It also makes the schema JSON-shaped, so it can come from a
config file or cross a worker boundary — a builder holding a `!Send` `Cif` cannot.

The one thing a builder buys that plain data does not is the layout query (`Point.offsetOf("y")`). That is
worth having — it is the calibration knob for `#pragma pack`, bitfields and `-fshort-enums`, which no computed
layout can see — but it does not need a builder. One free function, `layout({ struct: {...} })`, covers it.

The "build the CIF at the declaration line so a bad signature throws there" argument for builders is close to
worthless: both forms validate before any call happens, and an `open()`-time error that names the offending
**symbol key** is more useful than a bare throw at a declaration line.

### 3.2 The .d.ts

Full file at `/tmp/denplan/G-ffi/den-ffi.d.ts`. The load-bearing parts:

```ts
type NumberType = "i8"|"u8"|"i16"|"u16"|"i32"|"u32"|"f32"|"f64";
type BigIntType = "i64"|"u64"|"isize"|"usize";
interface StructType { readonly struct: { readonly [field: string]: ValueType } }
type ValueType  = NumberType | BigIntType | "bool" | "pointer" | "buffer" | StructType;
/** `buffer` is in-only: a returned pointer carries no length. */
type ResultType = Exclude<ValueType, "buffer"> | "void";

const pointerBrand: unique symbol;
interface Pointer<T = unknown> { readonly [pointerBrand]: T }

type Native<T> =
  T extends NumberType ? number :
  T extends BigIntType ? bigint :
  T extends "bool"     ? boolean :
  T extends "pointer"  ? Pointer | null :
  T extends "buffer"   ? Uint8Array<ArrayBuffer> :
  T extends "void"     ? void :
  T extends { struct: infer F } ? { -readonly [K in keyof F]: Native<F[K]> } :
  never;

type Bound<D> =
  D extends FnDef
    ? (...args: Args<D["params"]>) =>
        // Defensive tuple wrap. NOT load-bearing: `D["nonblocking"]` is an indexed
        // access, not a naked type parameter, so it never distributes (verified).
        [D["nonblocking"]] extends [true] ? Promise<Native<D["result"]>> : Native<D["result"]>
    : D extends StaticDef ? Native<D["type"]> : never;

// P and R must appear in the body or every Callback is assignable to every slot.
declare const sigBrand: unique symbol;
interface Callback<P extends readonly ValueType[], R extends ResultType> extends Disposable {
  readonly pointer: Pointer;
  readonly [sigBrand]: (p: P, r: R) => void;
}

type Symbols<S extends Schema> = {
  readonly [K in keyof S]: S[K]["optional"] extends true ? Bound<S[K]> | null : Bound<S[K]>
};
/** Symbols sit on the handle itself; the only reserved key is a well-known symbol,
 *  so a C identifier (`close`, `read`, `open`) can never collide. */
type Library<S extends Schema> = Symbols<S> & Disposable;

function open<const S extends Schema>(
  path: string | URL, schema: S, grant: FfiGrant,
): Library<S>;
```

Three details that matter, and one that turned out not to:

1. **`const S`** is what removes `as const` (same trick as `deno.d.ts:6821`). Without it, `params: ["i32"]`
   widens to `string[]` and arity checking evaporates — which is exactly Bun's state.
2. **The signature brand on `Callback<P, R>` is mandatory.** Without it, `P` and `R` never appear in the
   interface body, so every `Callback<...>` is structurally identical and any handle is assignable to any
   callback slot. Verified against the earlier unbranded `.d.ts` with both tsc 5.9.3 and 7.0.2
   (`/tmp/denplan/V2/ts/stress.ts`): both
   `lib.apply(callback({params:["f64","f64"],result:"f64"}, ...), 1)` and
   `lib.apply(callback({params:["i64"],result:"i64"}, ...), 1)` produced **no error** against a slot declared
   `{ callback: { params: ["i32"], result: "i32" } }` — the only diagnostic emitted was an unrelated
   implementation mismatch. Only the *implementation function* was ever typed against its own def; the
   handle-to-slot match was unchecked, which at run time is a wrong-signature libffi call, i.e. UB reachable
   from typechecking JS (fact 1). With the brand (`/tmp/denplan/V2/ts/den-ffi-brand.d.ts`) both cases error:
   `Argument of type 'Callback<readonly ["f64","f64"],"f64">' is not assignable to parameter of type
   'Callback<readonly ["i32"],"i32">'`. This is the tenth `@ts-expect-error` in `use.ts` (§3.3).
3. **`Native<T>` is direction-independent.** Deno needs a `ToNativeType`/`FromNativeType` pair
   (`deno.d.ts:6271` vs `:6335`) because its `buffer` and `struct` are asymmetric — a buffer argument is a
   TypedArray but a buffer *return* is a bare pointer. den forbids `buffer` as a result (`ResultType`), so one
   mapping covers both directions.
4. **The tuple wrap around `[D["nonblocking"]] extends [true]` is a no-op, kept only as defensive style.** An
   earlier draft listed it as mandatory, on the theory that an omitted `nonblocking` widens to `boolean` and a
   bare conditional distributes over that union. It does not: distributive conditional types trigger only on a
   *naked type parameter*, and `D["nonblocking"]` is an indexed access. Verified by removing it
   (`/tmp/denplan/V2/ts/den-ffi-notuple.d.ts`): `tsc -p tsconfig.notuple.json` over `use.ts` prints nothing
   (EXIT 0, all `@ts-expect-error` still consumed), and diffing the `flip.ts` diagnostics with and without the
   wrap gives `IDENTICAL DIAGNOSTICS with and without the tuple wrap`. Harmless to keep, wrong to teach.

**Polarity, settled: `nonblocking?: true` opts *in* to the off-thread call; sync is the default.** This is
Deno's polarity, and it is what the verified `.d.ts` at `/tmp/denplan/G-ffi/den-ffi.d.ts:33` (`readonly
nonblocking?: boolean`) and `:46` (`Bound = [D["nonblocking"]] extends [true] ? Promise<...> : ...`) actually
implements — there is no `sync` key anywhere in that file. An earlier draft asserted the opposite from §4.3
onwards (`sync: true` opting *out* of an async default), so the §3.3 exit-0 proof certified a schema the prose
rejected. The prose is the side that changed, for two independent reasons:

- **Async-by-default was asserted, never measured.** Every call would pay a `spawn_blocking` thread hop plus a
  fresh `ffi_prep_cif`, against an `add(i32, i32)` that costs a few nanoseconds. Only the second cost was ever
  defended ("free next to the thread hop", §4.3); the hop itself — the dominant one — was never priced. Both
  competitors default to sync.
- **The default was not what bought the safety.** The property that closes fact 6's deadlock is the
  marshal-time refusal in §4.7, and that rule is unchanged: a `Callback` may only be passed to a symbol
  declared `nonblocking: true`. A program that wants the off-thread call writes six characters and gets a
  `Promise`; a program that passes a callback has to write them.

### 3.3 Proof it infers

`tsc 5.9.3`, `strict` + `exactOptionalPropertyTypes` + `skipLibCheck: false`, over the `.d.ts` plus a call-site
file with nine `@ts-expect-error` directives and no `as const` anywhere. **This is the `nonblocking` schema of
§3.2, which is now also what §4 onwards specifies** — the proof and the design agree:

```
$ cd /tmp/denplan/G-ffi && npx -y -p typescript@5.9 tsc -p tsconfig.json; echo "EXIT=$?"
EXIT=0
```

Exit 0 with `@ts-expect-error` present proves each directive is *consumed* — an unused one is itself an error
(TS2578). Stripping the nine directives (`flip.ts`) yields exactly the nine intended diagnostics, verbatim:

```
flip.ts(27,5): error TS2554: Expected 2 arguments, but got 1.
flip.ts(28,9): error TS2345: Argument of type 'string' is not assignable to parameter of type 'number'.
flip.ts(29,19): error TS2353: Object literal may only specify known properties, and 'z' does not exist in type '{ x: number; y: number; }'.
flip.ts(30,29): error TS2345: Argument of type 'number' is not assignable to parameter of type 'bigint'.
flip.ts(31,7): error TS2322: Type 'Promise<bigint>' is not assignable to type 'bigint'.
flip.ts(32,7): error TS2322: Type 'Pointer<unknown> | null' is not assignable to type 'number'.
  Type 'null' is not assignable to type 'number'.
flip.ts(33,1): error TS2721: Cannot invoke an object which is possibly 'null'.
flip.ts(34,11): error TS2345: Argument of type '(n: number) => number' is not assignable to parameter of type 'Callback<readonly ["i32"], "i32">'.
flip.ts(35,45): error TS2322: Type '"buffer"' is not assignable to type 'ResultType'.
```

Line by line: wrong arity; `i32` is a number not a string; unknown struct field; `usize` demands a bigint;
a `nonblocking` result is a Promise; a pointer is not a number; an `optional` symbol may be null; a bare
function is not a `Callback` handle; `buffer` is not a result type.

**A tenth directive is owed and not yet in the file.** The reference `.d.ts` declares
`interface Callback<P, R> extends Disposable { readonly pointer: Pointer }` (`den-ffi.d.ts:54-56`) — `P` and `R`
are unused, so *any* callback handle satisfies *any* callback slot (§3.2 detail 2 has the two probe cases and
their non-diagnostics). Adding the `sigBrand` field fixes it, verified in
`/tmp/denplan/V2/ts/den-ffi-brand.d.ts`, and `use.ts` then needs a tenth `@ts-expect-error` on a
wrong-signature handle. Until both land, §2.3's "Callback typed" claim is only about the implementation
function, and the handle-to-slot match is an unchecked wrong-signature libffi call.

The call site that produces exit 0 (`/tmp/denplan/G-ffi/use.ts`), unannotated:

```ts
using lib = open(`./libprobe${suffix}`, {
  add:     { params: ["i32", "i32"], result: "i32" },
  scale:   { params: [{ struct: { x: "i32", y: "i32" } }, "i32"],
             result:  { struct: { x: "i32", y: "i32" } } },
  hash:    { params: ["buffer", "usize"], result: "u64", nonblocking: true },
  apply:   { params: [{ callback: { params: ["i32"], result: "i32" } }, "i32"], result: "i32" },
  alloc:   { params: ["usize"], result: "pointer" },
  maybe:   { params: [], result: "void", optional: true },
  version: { type: "i32" },
}, grant);

const a: number = lib.add(1, 2);                            // number
const p: { x: number; y: number } = lib.scale({x:1,y:2}, 3);// struct in AND out
const h: bigint = await lib.hash(new Uint8Array(4), 4n);    // Promise<bigint>
const v: number = lib.version;                              // static symbol
using cb = callback({ params: ["i32"], result: "i32" }, (n) => n * 2);
const r: number = lib.apply(cb, 21);
lib.maybe?.();                                              // optional -> `| null`
```

### 3.4 Known miss: option-key typos are not caught at compile time

```ts
// NOT an error: Schema's index signature suppresses excess-property checking.
open("x", { f: { params: ["i32"], result: "i32", nonblokcing: true } }, grant);
```

`Schema = { readonly [name: string]: SymbolDef }` has an index signature, so the nested per-symbol literal is
never freshness-checked. The obvious fix — making the parameter a mapped type
`schema: { readonly [K in keyof S]: SymbolDef }` — restores excess-property checking and **destroys inference**:
`S` falls back to its constraint and every symbol collapses to the full `Native<...>` union. Measured on the same
file that otherwise exits 0 (`/tmp/denplan/V2/ts/tsconfig.mapped.json`, tsc 5.9.3). The run emits **14
diagnostics**; this is a four-line excerpt — the first line is the one that demonstrates the fix working, the
rest are the collapse it costs:

```
use.ts(46,63): error TS2353: Object literal may only specify known properties, and 'nonblokcing' does not exist in type 'SymbolDef'.
use.ts(15,19): error TS2721: Cannot invoke an object which is possibly 'null'.
use.ts(15,23): error TS2349: This expression is not callable.
  Not all constituents of type 'number | bigint | boolean | Uint8Array<ArrayBuffer> | Pointer<unknown> | …' are callable.
    Type 'number' has no call signatures.
```

(The reference file carries a stale comment about exactly this at `/tmp/denplan/G-ffi/den-ffi.d.ts:61-62` —
*"The mapped-type parameter is what restores excess-property checking on each per-symbol literal; a bare
`schema: S` loses it to Schema's index signature"* — sitting directly above a signature that **is** `schema: S`.
Delete it when the file is next touched; as written it asserts the opposite of this section's conclusion.)

Not worth a workaround. den throws `FfiError { kind: "Schema" }` naming the unknown key at `open()`, which is
the backstop anyway and is what makes adding `packed`/`variadic` later a non-breaking change. A builder form
*would* catch this at compile time — it is the only concrete thing a builder buys, and it is not worth twelve
extra runtime exports.

### 3.5 The Rust side reads the same value

No new machinery: this is den's existing options-bag idiom one level deeper.
`den-stdlib-fs/src/lib.rs:124-131` is the shape:

```rust
impl<'js> FromJs<'js> for WriteOptions {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        Ok(Self { atomic: Object::from_js(ctx, value)?.get::<_, Option<bool>>("atomic")?.unwrap_or_default() })
    }
}
```

An `FnSpec` is the same with more fields plus a `TryFrom<&str>` for `NativeType`. Unknown keys throw rather
than being ignored — [roadmap rule 4](14-runtime-feature-roadmap.md), and precisely the failure mode Bun's
silently-ignored `nonblocking` demonstrates.

---

## 4. The chosen design

### 4.1 Crate layout

Wired exactly like `den-stdlib-sqlite`, which is the closest existing precedent (a native class, an optional
crate, a cargo feature):

```
den-stdlib-ffi/
  Cargo.toml        den-util, rquickjs {macro,futures,array-buffer}, tokio {rt,sync}, dlopen2, libffi
  src/lib.rs        js_ffi module def; `open`, `suffix`, `layout`, FfiError
  src/schema.rs     NativeType / ParamSpec / FnSpec / StructLayout + FromJs
  src/grant.rs      FfiGrant userdata + the single capability check
  src/library.rs    Library class, open/dispose, symbol binding, sync + nonblocking call
  src/marshal.rs    JS <-> raw bytes; the BigInt guard; buffer rules
  src/pointer.rs    Pointer class + the `ptr` namespace
  src/callback.rs   Callback class, Slot, trampoline, realm pump
  tests/ffi.rs + tests/c/probe.c
den-util/src/lib.rs   gains `OwnedCtx`, moved from den-stdlib-wasm/src/backend.rs:98-122 (phase 4 only, §1.4)
```

Wiring, five one-line edits mirroring sqlite: `Cargo.toml:14` (members), `:49` (workspace dep), `:713`
(`stdlib-ffi = ["den-core/stdlib-ffi"]`), `den-core/Cargo.toml:44` (optional dep) and `:101`
(`stdlib-ffi = ["dep:den-stdlib-ffi"]`). Registration in `den-core/src/engine.rs`, copying the sqlite
`cfg` blocks at `:289-291` (`resolver.with_module("den:sqlite")`) and `:380-382`
(`loader.with_module("den:sqlite", den_stdlib_sqlite::js_sqlite)`).

**Unlike `stdlib-sqlite` (`den-core/Cargo.toml:85`), `stdlib-ffi` is NOT a member of the `stdlib` umbrella**
and not in `default`. It is opt-in at compile time, and even then denied at run time without a grant.
No global is installed — den:ffi is import-only.

### 4.2 Schema in Rust

```rust
#[derive(Clone)]
enum NativeType { I8,U8,I16,U16,I32,U32,I64,U64,Isize,Usize,F32,F64,
                  Bool, Pointer, Buffer, Void, Struct(Rc<StructLayout>) }
struct StructLayout { fields: IndexMap<String, NativeType>, offsets: Vec<usize>, size: usize, align: usize }
enum ParamSpec { Value(NativeType), Callback(Arc<FnSig>) }
struct FnSig  { params: Vec<NativeType>, result: NativeType }   // plain data: Send + Sync
struct FnSpec { symbol: CString, params: Vec<ParamSpec>, result: NativeType, nonblocking: bool }
```

`FnSig` exists separately from `FnSpec` for one reason: it is the `Send` twin. The cached `Cif` is `!Send`
(fact 4), so both the `spawn_blocking` path and the foreign-thread callback path need a plain-data description
they can carry across a thread and rebuild a `Cif` from on the far side.

Struct layout is computed in Rust — `offset = align_up(cursor, align_of(field)); cursor += size_of(field)`,
`align = max(field aligns)`, `size = align_up(cursor, align)` — and then **cross-checked** against
`middle::Type::structure(...).size()`. A mismatch is `FfiError { kind: "Layout" }` at `open()`. That is the
self-check for the only arithmetic in the crate, and it converts a wrong ABI on an untested target from silent
corruption into an error before any call happens. `layout(type)` exposes `{ size, align, offsets }` to JS for
the cases arithmetic cannot see (§5.2).

Named fields also make Deno's unchecked-struct hole (§2.1) unrepresentable: there is no caller-supplied buffer
to be the wrong size.

### 4.3 Library and the call

```rust
#[rquickjs::class] pub struct Library { #[qjs(skip_trace)] inner: Rc<LibraryInner> }
struct LibraryInner { lib: dlopen2::raw::Library, live: Arc<AtomicBool>, path: PathBuf }
struct BoundFn {
  addr: *const c_void,           // dlopen2 raw/common.rs:154
  cif:  libffi::middle::Cif,     // !Send — never leaves the realm thread
  sig:  Arc<FnSig>,              // the Send twin
  spec: Rc<FnSpec>,
  lib:  Rc<LibraryInner>,        // keeps pages mapped; carries `live`
}
```

dlopen2 returns a bare `T` from `symbol`, with no borrow tying it to the `Library` — so the `Rc<LibraryInner>`
in each `BoundFn` *is* the tie, and `live` is what turns "JS calls a symbol after dispose" into a typed throw
instead of the SIGSEGV of fact 12. `open_with_flags` (`raw/common.rs:100`) makes `RTLD_NOW | RTLD_LOCAL`
explicit; neither Deno nor Bun exposes this.

**Sync call (the default):** check `live` → marshal each argument into an owned cell → build the `Ret` and call
`cif.call_return_into(CodePtr(addr), &args, libffi::middle::Ret::new(&mut cell))` → marshal the return. Note the
third parameter's type: `pub unsafe fn call_return_into(&self, fun: CodePtr, args: &[Arg], ret: Ret)`
(`libffi-5.2.0/src/middle/mod.rs:345`) takes a `middle::Ret`, **not** a `*mut u8`. The cell is exactly
`result.size()` bytes — `call_return_into` corrects sub-register returns itself and writes no more than that
(fact 3), which is the whole reason a dynamic marshaller can use it.

**Nonblocking call, `nonblocking: true`:** marshal into owned `NativeValue`s, send
`(addr as usize, Arc<FnSig>, args)` — all `Send` — into `tokio::task::spawn_blocking`, and rebuild the `Cif`
**there** from the plain-data `FnSig`. One extra `ffi_prep_cif` per off-thread call is free next to the thread
hop; the thread hop itself is not free, which is why it is opt-in (§3.2). den already uses `spawn_blocking` this
way at `den-stdlib-fs/src/lib.rs:107`.

Sync is the **default** and `nonblocking: true` opts in — Deno's polarity, and the one the `.d.ts` verifies
(§3.2). What makes callbacks safe is not the default; it is the marshal-time refusal of §4.7.

**One constraint on any future argument-cell caching**, recorded here because it belongs next to the
struct-by-value section and is easy to trip over later. `libffi-5.2.0/src/low.rs:411-413`, verbatim:
*"libffi modifies some of the pointers in args if the struct is large enough. It copies large structures to a
new location and rewrites the pointer. this leads to an issue if args is being reused across multiple calls."*
The design marshals a fresh argument array per call, so it is safe today — but a per-symbol cached cell array,
which is the obvious optimisation once someone profiles, is **not** safe for any signature containing a
struct-by-value parameter.

**dlopen flags are fixed, not exposed.** `open_with_flags` (`dlopen2/src/raw/common.rs:100`) pins
`RTLD_NOW | RTLD_LOCAL`: `NOW` so a missing symbol is an `open()`-time error rather than a lazy SIGSEGV, `LOCAL`
so a loaded library cannot satisfy another library's undefined symbols. den *uses* explicit flag control that
neither Deno nor Bun has, but does not surface it — 15 §3.18's P3 row *"dlopen flags at load time
(RTLD_LAZY/NOW/GLOBAL/LOCAL/DEEPBIND)"* is **deferred**, and `open()` takes no flags argument. Adding one later
is non-breaking because unknown schema keys throw today.

### 4.4 Marshalling matrix

| Schema type | JS value | As argument | As result | Notes |
|---|---|---|---|---|
| `i8 u8 i16 u16 i32 u32` | `number` | range-checked, truncating conversion refused | read from an exactly-sized cell, sign-extended in Rust (fact 3) | out-of-range throws `Range` |
| `f32 f64` | `number` | direct | direct | `f32` narrows silently, as C does |
| `i64 isize` | `bigint` | decimal string → `i64::from_str` | `BigInt::from_i64` | never `to_i64` (fact 8) |
| `u64 usize` | `bigint` | decimal string → `u64::from_str` | `BigInt::from_u64` | never `asUintN` (fact 8) |
| `bool` | `boolean` | `u8` 0/1 | `!= 0` | |
| `pointer` | `Pointer \| null` | address + provenance check | `null` if 0, else `Pointer` carrying the library's `live` | never a number, never a bigint |
| `buffer` | `Uint8Array` | `as_raw()` (`RawArrayBuffer{len,ptr}`), byteOffset-correct, borrowed for exactly one `Cif::call` | **forbidden** (`ResultType`) | detached → `BadArgument`; shared/resizable need their own check (§4.6); sync symbols only |
| `{ struct: {...} }` | plain object | fields written at computed offsets into an owned cell | fresh object read from the returned cell | missing field → `Schema` naming it |
| `{ callback: {...} }` | `Callback` handle | closure code pointer | n/a | allowed **only** on a `nonblocking: true` symbol (§4.7) |
| `void` | `undefined` | n/a | `undefined` | |

The 64-bit rows are the ones people will want to "optimize". Do not. A decimal-string round-trip per 64-bit
argument is nothing next to a foreign call, and it is the only path that is correct through *both* engine bugs
in fact 8 without depending on either being fixed.

```rust
// ponytail: BigInt via its decimal string. Exact, and immune to both
// `BigInt::to_i64`'s silent wrap and quickjs-ng's broken `asUintN`
// (doc 09 fact 12). Swap for a limb API only if a profile ever shows it.
```

### 4.5 Pointers

```rust
#[rquickjs::class] pub struct Pointer { addr: usize, live: Arc<AtomicBool> }
```

Opaque, unforgeable from JS, and **provenance-carrying**: every `Pointer` holds the liveness flag of the library
that produced it, so `ptr.view(p, n)` after `lib[Symbol.dispose]()` throws `FfiError { kind: "Closed" }` instead
of reading unmapped pages. Deno has the opaque object but no provenance; Bun has a JS number and therefore
neither.

**Say plainly what that is not.** Two claims an earlier draft made are stronger than the design can deliver, and
overstating them here would be the same sin this section correctly convicts `ptr.create` of:

- **The mandatory `byteLength` on `ptr.view` is an ergonomic requirement, not a bounds check.** den cannot know
  how large the pointee is — a `.so` carries no such information (§1.1) — so `ptr.view(livePointer, 1 << 30)` is
  an arbitrary read that passes every check in §5.2, whose `RangeError` row validates only that the argument is
  a number. Removing `ptr.create`/`ptr.offset` closes the *forgery* door; it does not close this one. The value
  of the mandatory length is that the resulting `Uint8Array` has a length at all, so every read *after* the view
  is bounds-checked by the engine — Deno's `UnsafePointerView` never gets even that far.
- **`live` tracks exactly one thing: den's own `dlclose`.** A pointer the library itself `free`d, a pointer into
  a returned stack frame, and a pointer the library invalidated on its own schedule all still read `live == true`
  and pass every stated check. Fact 12 is the reproduction (`/tmp/denplan/V2/rust/src/bin/unload.rs`,
  `[exit=139]`) and §5.1 carries this as the **fourth** thing den cannot fix.

The JS namespace is deliberately four functions:

```ts
namespace ptr {
  function value(p: Pointer): bigint;
  function equals(a: Pointer | null, b: Pointer | null): boolean;
  function view(p: Pointer, byteLength: number): Uint8Array<ArrayBuffer>;   // length MANDATORY
  function cstring(p: Pointer, maxByteLength?: number): string;            // bounded NUL scan
}
```

There is **no `create(bigint)` and no `offset(p, n)`**. An earlier draft had both, copying Deno's
`UnsafePointer.create`/`offset` — and they compile this, with no diagnostic, against that draft's own `.d.ts`:

```ts
const forged = ptr.create(0x7fff_0000_0000n)!;
const bytes  = ptr.view(forged, 4096);        // arbitrary read
const walked = ptr.offset(forged, 1 << 20);   // pointer arithmetic
```

A pointer minted from an integer has no owning library, so there is no liveness flag to check and no
provenance to enforce — which makes the "opaque and unforgeable" claim false in the same file that asserts it.
`view` without a length is Deno's `UnsafePointerView`, which structurally cannot bounds-check anything
(`deno.d.ts:6536`, the constructor takes no length). Both are removed. If a script genuinely needs to walk a
buffer, it walks the `Uint8Array` that `view` returned.

### 4.6 Buffers

A sync symbol **borrows**. The handle is `TypedArray::<u8>::as_raw()` →
`RawArrayBuffer { len: usize, ptr: NonNull<u8> }` (`typed_array.rs:213`, `array_buffer.rs:57-60`), **not**
`as_bytes()`: `as_bytes` returns `&[u8]` (`typed_array.rs:208-211`), which cannot back a C function that writes
into the buffer, and `fill_bytes(view, 8)` in phase 3 is exactly that. Both are byteOffset-correct and both go
through `get_raw_bytes`. The pointer is valid for exactly the duration of one `Cif::call` and is never stored.

**Three buffer hazards, three different mechanisms — they do not come free with one call.**

| Hazard | Detected by | Cost |
|---|---|---|
| detached | `as_raw()`/`as_bytes()` returning `None` (`get_raw_bytes`, `typed_array.rs:253-275`) | free |
| **shared** (`SharedArrayBuffer`) | an explicit JS-side check — rquickjs 0.12.2's `ArrayBuffer` API (`array_buffer.rs:230-304`: `len`, `is_empty`, `as_bytes`, `as_slice`, `detach`, `as_value`, `as_raw`) has no `is_shared` | one property read per marshal |
| **resizable** (`maxByteLength`) | likewise absent from the API; read `buf.resizable` | one property read per marshal |

Both hazards are live in den today — `den bufs.js` prints `SharedArrayBuffer: function` and
`new ArrayBuffer(8, { maxByteLength: 16 }).resizable === true` — and a shared or resizable backing store can be
moved or mutated by another agent *while C holds the pointer*, so refusing them is not optional. The mechanism
must be named or it will not be built: read `view.buffer`, check its constructor against the realm's
`SharedArrayBuffer` and read its `resizable` property, and throw `FfiError { kind: "BadArgument" }` on either.
If that per-marshal cost is ever unacceptable, the alternative is a raw `JS_GetTypedArrayBuffer`/
`JS_GetAnyOpaque` call, not dropping the check.

`buffer` on a `nonblocking: true` symbol is an `open()`-time `FfiError { kind: "Schema" }`. Copying in and
silently not copying back is exactly the failure mode roadmap rule 4 forbids; the escape hatch is a pointer from
the library's own allocator. So: **zero-copy is a sync-symbol-only capability**, stated up front rather than
discovered.

Neither Deno nor Bun documents this contract at all — `grep -i 'keep.*alive|garbage|lifetime|detach'` over
`deno.d.ts` returns zero hits in the FFI category, and Bun's `ptr()` doc block (ffi.d.ts:995-1011) mentions
only performance and byteOffset.

### 4.7 Callbacks and the foreign-thread seam

Userdata is plain data plus a parked context — the wasm pattern of §1.4, split by who may touch what:

```rust
#[derive(JsLifetime, Default)]
struct FfiRealm<'js> {                                   // context userdata, NEVER Box::leak (fact 11)
  functions: RefCell<Vec<Function<'js>>>,                // == ImportedFunctions, instance.rs:40
  closures:  RefCell<Vec<CallbackEntry>>,
}

/// DROP ORDER IS LOAD-BEARING. Struct fields drop in declaration order, so the
/// `Closure` -- which holds a raw userdata pointer into `slot` -- must be
/// declared FIRST and therefore dropped FIRST. An earlier draft wrote this as
/// `(Box<Slot>, Box<Closure<'static>>)`, which drops the pointee first.
struct CallbackEntry {
  closure: libffi::middle::Closure<'static>,
  slot:    std::pin::Pin<Box<Slot>>,       // address-stable: never moved, never realloc'd
}

impl CallbackEntry {
  fn new(cif: libffi::middle::Cif, slot: Slot) -> Self {
    let slot = Box::pin(slot);
    // SAFETY: the one unsafe line the container costs, and the alternative to
    // `Box::leak`, which fact 11 forbids. `slot` is pinned behind a Box, so its
    // address is stable for this struct's whole life; `closure` is declared
    // first and so is dropped first, so the borrow never outlives the pointee;
    // and nothing hands out a `&Slot` that could escape.
    let userdata: &'static Slot = unsafe { &*(&*slot as *const Slot) };
    Self { closure: libffi::middle::Closure::new(cif, trampoline, userdata), slot }
  }
}

struct Slot { shared: Arc<SharedSlot>, local: LocalSlot }
struct SharedSlot {                       // touched from ANY thread: Send + Sync
  owner: std::thread::ThreadId, live: AtomicBool,
  mailbox: tokio::sync::mpsc::UnboundedSender<CallRequest>,
  sig: Arc<FnSig>,                        // plain data, NOT Rc<FnSpec> — an Rc refcount would race
}
struct LocalSlot { index: usize, reentrant: den_util::OwnedCtx }   // only when owner == current thread
```

The self-reference is not avoidable and must not be hidden. `Closure::new(cif, callback, userdata: &'a U)`
(`libffi-5.2.0/src/middle/mod.rs:443`) **borrows** its userdata, and `Closure<'a>` carries
`PhantomData<&'a ()>` (`:422-427`), so a `Closure<'static>` demands a `&'static Slot`. A sibling `Box<Slot>` in
the same struct cannot supply one; that shape does not compile
(`/tmp/denplan/V2/rust/src/bin/realm_struct.rs`):

```
error[E0597]: `*slot` does not live long enough ... type annotation requires that `*slot` is borrowed for `'static`
error[E0505]: cannot move out of `slot` because it is borrowed
```

The only two exits are `Box::leak` — forbidden by fact 11, which aborts the process at shutdown — and one
documented lifetime widening behind a stable address, which is what `CallbackEntry::new` is. Write it once, in
one place, with the SAFETY comment and the drop-order comment; there is no third option.

The trampoline, wrapped whole in `catch_unwind` (fact 7), branches on
`std::thread::current().id() == slot.shared.owner`:

**Same thread** — C called back synchronously inside a call we made. Re-enter directly through `OwnedCtx::with`:
we already hold the runtime lock, which is exactly the precondition `backend.rs:112-119` documents. R7 seam
case c, exit 0.

**This branch is much rarer than "qsort, bsearch, visitor APIs; the common case" suggests, and under the
design's own rules those APIs never reach it.** A `Callback` may only be passed to a `nonblocking: true` symbol
(the refusal below), and a `nonblocking` call runs under `spawn_blocking` — so C invokes the trampoline on a
tokio blocking thread, `thread::current().id() != slot.shared.owner`, and an inline comparator takes the
**foreign** branch. The performance consequence is not small and the doc must state it: an
`n log n` sort costs one mailbox round-trip **plus a realm-pump wakeup per comparison**, which is
microseconds each against a comparator that should cost nanoseconds. den:ffi is not the tool for a hot
comparator; the honest advice is to sort in JS, or to sort in C with a C comparator. The same-thread branch is
reachable only for a callback registered earlier through a `nonblocking` symbol, stored by C, and fired later
from the realm thread — which is also the case the residual-hole paragraph below is about. It is not dead code,
but it is not the common case, and it is a thin second user for the `OwnedCtx` move of §1.4.

**Foreign thread** — marshal the raw arguments through `Arc<FnSig>` into owned `NativeValue`s, post a
`CallRequest`, `blocking_recv` the reply. On the realm side one `ctx.spawn`-ed pump services the mailbox:

```rust
ctx.spawn(async move {                                   // doc 09 fact 4: the only legal way in
  while let Some(req) = rx.recv().await {
    let f = FfiRealm::get(&ctx, req.index)?;
    let _ = req.reply.send(call_js(&ctx, &f, req.args));
  }
});
```

R7 seam case a, against a real realm:

```
a realm thread = ThreadId(1)
[cb on ThreadId(4)] FOREIGN thread -> posted to realm, blocking
[pump on ThreadId(1)] JS returned 307
a idle() -> Err(Elapsed(())) (pump future still alive keeps it open)
a foreign thread result = Ok(307)
```

That `idle()` line is the second half of the design and it is free: by doc 09 fact 3 and
[ARCHITECTURE §7.5 rule 1](../../ARCHITECTURE.md) — *"A queue the script opened stays open until the script
closes it"* — a live pump keeps the process alive, and `Callback[Symbol.dispose]()` ends it. This is the same
mechanism `den-stdlib-worker/src/port.rs:210-217` calls *"the process-lifetime mechanism for ports"*. So
den:ffi needs **no `ref()`/`unref()` and no `threadSafe()` concept**, and neither Deno's silent drop nor Bun's
crash is reachable.

**The deadlock, and how den refuses it.** Fact 6: a sync symbol whose C function spawns a thread, calls back and
joins hangs for ever. den cannot detect that in advance — nothing tells it what a C function will do. So it does
not try:

> **Passing a `Callback` to a symbol not declared `nonblocking: true` throws
> `FfiError { kind: "BadArgument" }` at marshal time.**

Deterministic, checked on the realm thread, no race, no false positives, and one word in the schema to satisfy.
R7 seam case d proves the `nonblocking` path works with the *same* C function that deadlocks case b:
`d JS calls the async wrapper; the realm thread will park in idle()` / `[cb on ThreadId(5)] FOREIGN thread ->
posted to realm, blocking` / `[pump on ThreadId(1)] JS returned 307` / `d JS got 307`, exit 0.

An earlier draft used an `in_sync_call: AtomicBool` checked from the foreign-thread branch instead. It is
worse in two ways and was dropped: check-then-post is not atomic with the realm entering a sync call, so the
deadlock is narrowed rather than closed; and "report the error" means touching JS from the one branch that may
not. Its third supposed flaw — that it converts a self-resolving stall into a zero-filled return — turns out to
be the *correct* behaviour, for the reason immediately below.

**The residual hole is an unbounded deadlock, not a bounded stall, and the marshal-time refusal cannot see it.**
An earlier draft claimed it was "bounded by the sync call, not infinite". It is not. Probe
`/tmp/denplan/V2/rust/src/bin/residual.rs` against `c/probe.c`'s `fire_on_thread_and_join()`
(`pthread_create` → fire the stored callback → `pthread_join`). The callback is registered through *one* symbol
and is never passed as an argument to the sync symbol, so the refusal rule never fires — there is nothing at
that call site to refuse. Under `timeout 6`:

```
R1 callback registered; realm thread ThreadId(1) now enters a SYNC symbol
   [cb on ThreadId(2)] FOREIGN thread -> posting to realm mailbox, blocking
[exit=124]
```

The realm thread is parked inside the C frame that is joining the thread that is waiting on the realm's mailbox.
It does not resolve when the sync call returns, because the sync call **cannot** return. This is fact 6's
deadlock, reached despite the refusal.

Two things follow, and both ship:

1. **The foreign-thread branch's `blocking_recv` is bounded, and the timeout is not a nicety.** On expiry it
   writes the zero C value, returns to C, and logs one line to stderr naming the library, the symbol and the
   timeout. That is what breaks the cycle: C's thread finishes, the `pthread_join` returns, the sync call
   returns, and the realm resumes. It trades an unrecoverable process hang for a bounded stall plus a wrong
   (zero) answer plus a diagnostic — which is the only trade available, since no JS call is legal from that
   thread and no check on the realm side can see the situation coming.
2. **§5.1 gains a fourth "thing den cannot fix".** Even with the timeout, a sync symbol can stall the realm for
   the whole timeout window, and the value C receives is a lie. Say so in the docs rather than let someone
   discover it.

### 4.8 Lifetime, disposal, errors

`Library` and `Callback` implement `Symbol.dispose` and nothing else — no `close()` method, so the **only**
reserved key on a `Library` is a well-known symbol and a C identifier called `close`, `open` or `read` can
never collide with it. That is why symbols hang off the handle directly rather than under a `.symbols`
namespace.

Disposal clears `live` **before** `dlclose`, so every subsequent JS-side dispatch — a symbol call, a pointer
deref, a callback invocation from JS — is `FfiError { kind: "Closed" }`. C-side use after dispose remains
undefined behaviour (fact 12) and is documented as the one thing den:ffi does not protect.

```ts
class FfiError extends Error {
  readonly kind: "NotCapable" | "Open" | "Symbol" | "Schema" | "Layout"
               | "Range" | "BadArgument" | "Closed";
  readonly path?: string;
  readonly symbol?: string;
}
```

One sum type with a `.kind` discriminant, per [15 §4.4](15-stdlib-parity-gap.md). No sentinel returns:
`Result<T, FfiError>` in Rust, a thrown `FfiError` in JS.

---

## 5. Safety

### 5.1 Unsafe by definition — five things den cannot fix

1. **A wrong schema is memory corruption.** A `.so` carries no type information, so
   `{ params: ["i32"], result: "i32" }` against `double f(char*)` is UB with no diagnostic available at any
   layer. The schema describes what the *script believes*; libffi trusts it. den's only mitigations are that
   the schema is one greppable value and that struct layout is computed rather than hand-packed — strictly
   better than Deno's unchecked positional buffers (§2.1), and infinitely better than nothing, but not a fix.
2. **C keeping a pointer past dispose.** Fact 12, twice: a dropped closure fired by C is SIGSEGV, a stale
   symbol address called after `dlclose` is SIGSEGV. If the library stored the callback pointer or spawned a
   thread that will call it later, freeing unmaps executable pages. den turns the *JS-side* case into a typed
   throw and can do nothing about the C-side case.
3. **FFI is arbitrary code execution.** A permission check does not make it safe. It makes it **auditable and
   off by default**, which is a different and achievable goal.
4. **`ptr.view` is an arbitrary read, and `live` is not a validity flag.** §4.5: den cannot know the pointee's
   size, so the mandatory `byteLength` is an ergonomic requirement rather than a bounds check —
   `ptr.view(livePointer, 1 << 30)` passes every check den has. And `live` tracks only den's own `dlclose`: a
   pointer the library `free`d, or one into a returned stack frame, still reads `live === true`. Removing
   `ptr.create`/`ptr.offset` closes the forgery door and only that door. Do not let §5.2's table read as a
   safety property it is not.
5. **A sync symbol can stall the realm, and hand C a wrong answer, when a stored callback fires into it.**
   §4.7's residual case: a callback registered through a `nonblocking` symbol, stored by C, and fired from a
   thread that the *sync* symbol then joins, is a cycle no marshal-time check can see (probe `[exit=124]`).
   The bounded `blocking_recv` breaks the cycle but pays for it with a stall of the timeout's length and a
   zero return handed to C. Documented, not fixed.

### 5.2 What den still checks

| Check | Where | Failure |
|---|---|---|
| capability | `Library::open`, the single check site | `NotCapable` with the resolved absolute path |
| unknown schema key | `FromJs for FnSpec` | `Schema` naming the key |
| `buffer` as a result | `.d.ts` `ResultType` + `FromJs` | compile error, then `Schema` |
| `buffer` on a `nonblocking` symbol | `open()` | `Schema` naming the symbol |
| struct layout vs libffi | `open()` | `Layout` |
| 64-bit range | every marshal | `Range` |
| **detached** buffer | every marshal, free (`as_raw()` → `None`) | `BadArgument` |
| **shared / resizable** buffer | every marshal, one explicit JS property read each — rquickjs exposes no `is_shared`/`resizable` (§4.6) | `BadArgument` |
| `Callback` to a symbol without `nonblocking: true` | marshal | `BadArgument` |
| use after dispose (symbol, pointer, callback) | every JS-side dispatch | `Closed` |
| panic in a trampoline | `catch_unwind` | zero C return + `report_exception` |
| JS throw in a callback | trampoline | zero C return + `report_exception` |
| foreign-thread callback with no realm progress | bounded `blocking_recv` (§4.7) | zero C return + one stderr line |
| pointer view length **present** (not in range — §5.1) | `ptr.view` | `RangeError` |
| unbounded C string | `ptr.cstring(p, max)` | `Range` |

Plus the calibration knob the physical world needs: `layout({ struct: {...} })` returns den's computed
`{ size, align, offsets }` so a script can `assert` against its actual header. No computed layout can see
`#pragma pack`, `__attribute__((packed))`, bitfields, anonymous unions or `-fshort-enums`; exposing what den
believes is the only way a caller can find out that den is wrong. A `packed` option is the obvious next knob,
and is deferred — which is a non-breaking change precisely because unknown keys throw today.

### 5.3 The minimum gate that ships now

Not a permission system. Three things:

1. `stdlib-ffi` — a cargo feature, **off by default**, not in the `stdlib` umbrella. A default `cargo build`
   never compiles libffi and never touches autoconf.
2. `FfiGrant { roots: Arc<[PathBuf]> }` — a value in context userdata, minted only by the CLI from
   `--allow-ffi[=PATH…]`. `src/main.rs:12-54` is a hand-rolled parser (`struct Cli { file, repl }`, string
   matching, no clap), so this is one more `else if` branch; storage follows the existing
   `Engine::store_userdata` pattern.
3. `open(path, schema, grant)` takes the grant **as an argument**. That is the difference between a capability
   and a process-global flag: a module that imports `den:ffi` without being handed a grant cannot bind
   anything. `open` canonicalizes the path and checks containment in a root; failure is
   `FfiError { kind: "NotCapable", path }`.

Workers receive the parent's grant or nothing — never widened.

One check site, one error kind, one userdata slot. That is the seed [15 §3.19](15-stdlib-parity-gap.md) asks
for (*"a single Rust Capabilities value in realm userdata with one check site per axis and a typed
CapabilityError, narrowed on Worker creation"*), and the eventual `den:permissions` inherits it rather than
replacing it. It has **no revocation, no per-symbol scoping, no prompt, and no audit log**, and it is only as
good as whoever wires the composition root — which today is `Engine::new`'s fixed shape, because
`EngineBuilder` does not exist yet. That is one more reason the ordering in fact 0 is right.

---

## 6. Implementation plan

Six phases. Each is independently shippable and each names the regression its test catches.

### Phase 1 — call `int add(int, int)` from a `.so`

**Files:** `Cargo.toml` (members `:14`, workspace dep `:49`, feature `:713`), `den-core/Cargo.toml` (`:44`,
`:101`), `den-core/src/engine.rs` (two `cfg` blocks at `:289` and `:380`), `src/main.rs` (`--allow-ffi`),
`den-stdlib-ffi/{Cargo.toml,src/lib.rs,src/schema.rs,src/library.rs,src/grant.rs,src/error.rs}`,
`den-stdlib-ffi/tests/{ffi.rs,c/probe.c}`, `types/den-ffi.d.ts`.

**Deliverable:** `open(path, schema, grant)` returning a `Library` whose function symbols are callable.
`i32`, `f64`, `void` only; sync only (`nonblocking` is phase 5). `Symbol.dispose`. The grant check site is
live. The `.d.ts` ships with the `Callback<P, R>` signature brand of §3.2 from the start, even though callbacks
are phase 4 — adding a brand to a published interface later is a breaking change.

**Test:** `tests/ffi.rs` builds `tests/c/probe.c` with
`Command::new(env::var("CC").unwrap_or("cc".into())).args(["-shared","-fPIC","-o", &so, …])` into a tempdir
(skip with a printed reason if `cc` is absent), then from JS asserts: `lib.add(35, 7) === 42`;
`lib.mul_f64(1.5, 2.0) === 3.0`; open without a grant → `kind === "NotCapable"`; missing path → `"Open"`;
bogus symbol → `"Symbol"`; and after `lib[Symbol.dispose]()`, `lib.add(1,1)` throws `"Closed"` **rather than
segfaulting**.

### Phase 2 — the rest of the scalar vocabulary

**Files:** `src/schema.rs`, `src/marshal.rs`, `src/pointer.rs`, `src/library.rs`, `types/den-ffi.d.ts`.

**Deliverable:** all integer widths, `bool`, `f32`, `pointer`, static symbols (`{ type }`), `optional: true`
→ `null`, `name` override, `suffix`, and the `ptr` namespace (`value`/`equals`/`view`/`cstring`; `view`'s
length mandatory, and **not** a bounds check — §5.1). 64-bit through the decimal string. Return cells sized
exactly `result.size()`, since `call_return_into` corrects sub-register returns itself (fact 3).

**Test:** round-trip `u64::MAX` and `i64::MIN` through the `.so` and assert exact equality — this single
assertion catches both engine bugs in fact 8. `2n**70n` as a `u64` argument → `"Range"`. An `i8` return of
`-1` comes back as `-1`, and an adjacent canary byte is **unchanged** — the assertion that an exactly-sized
cell is right and that nothing widened into its neighbour (fact 3). `ptr.cstring(p, 4)` over an unterminated
8-byte region → `"Range"`. A missing `optional` symbol is `null` while a missing required one fails `open()`.

### Phase 3 — buffers

**Files:** `src/marshal.rs`, `src/schema.rs`.

**Deliverable:** `buffer` arguments via **`as_raw()`** (`RawArrayBuffer { len, ptr }`) — not `as_bytes()`, which
is `&[u8]` and cannot back a C function that writes — byteOffset-correct, borrowed for one call, detached →
typed throw. Shared and resizable buffers refused through their own explicit property reads (§4.6); rquickjs
0.12.2 gives no `is_shared`/`resizable` and both are constructible in den today. `buffer` on a `nonblocking`
symbol or as a result is an `open()`-time `"Schema"` error.

**Test:** `fill_bytes(view, 8)` through a `new Uint8Array(buf, 2)` view; the parent buffer shows the write at
offset 2 (proves byteOffset is honoured **and** that the pointer is writable, which `as_bytes()` would not have
given). Detached, `SharedArrayBuffer`-backed and `{maxByteLength}`-resizable buffers each throw
`"BadArgument"` rather than being silently accepted. A schema with a `nonblocking` `buffer` parameter fails at
`open()` naming the symbol.

### Phase 4 — callbacks, same-thread only

**Files:** `den-util/src/lib.rs` (`OwnedCtx` moves in), `den-stdlib-wasm/src/backend.rs` (re-export),
`src/callback.rs`, `src/lib.rs`.

**Deliverable:** `callback(def, fn)` → a handle whose `.pointer` is a real C function pointer. `FfiRealm`
userdata holds the registry and the `CallbackEntry` values of §4.7 — `Closure` declared first so it drops
first, `Pin<Box<Slot>>` for a stable userdata address, one documented lifetime widening, never `Box::leak`.
Trampoline in `catch_unwind`; a JS throw becomes the zero C value plus `report_exception`. Foreign-thread
invocation returns zero and logs in this phase; upgraded in phase 5. A `Callback` passed to a symbol without
`nonblocking: true` already throws. `OwnedCtx` moves to `den-util` **here** and not before (§1.4): this phase is
its second user.

**Test:** `apply(cb, 21) === 42`; a qsort-shaped comparator sorts an array (and note the cost — §4.7: once
phase 5 lands, every comparison is a mailbox round-trip, because the symbol must be `nonblocking` to accept the
callback at all); a callback that throws does not
abort the process and the C function still returns (assert exit 0 and that the exception was reported); and —
the one that catches fact 11 — **drop a realm with a live `Callback` registered and assert exit 0**, not
`JS_FreeRuntime: Assertion 'list_empty(&rt->gc_obj_list)' failed`.

### Phase 5 — nonblocking calls and the foreign-thread seam

**Files:** `src/library.rs`, `src/callback.rs`.

**Deliverable:** `nonblocking: true` symbols: arguments marshalled to owned values, `(addr, Arc<FnSig>, args)`
into `spawn_blocking`, `Cif` rebuilt there, Promise resolved on return. Foreign-thread callbacks route through
the mailbox serviced by a `ctx.spawn`-ed pump; the pump keeps `idle()` pending while a callback is armed and
is cancelled by dispose. The foreign-thread `blocking_recv` is **bounded** (§4.7): on expiry it returns the zero
C value and writes one stderr line naming library, symbol and timeout. That bound is what turns the residual
deadlock into a stall.

**Test:** a C function that `pthread_create`s, sleeps 200 ms, calls back and joins — via a `nonblocking`
symbol it resolves with the callback's value; a `Callback` passed to the sync variant throws `"BadArgument"` at
the call, so the test finishes instead of timing out. Plus: with only an armed callback outstanding the process
does **not** exit, and exits within a tick of `cb[Symbol.dispose]()`. And the one that catches §4.7's residual
case: a callback stored by C and fired from a thread a **sync** symbol then joins — the process must recover at
the `blocking_recv` timeout with a stderr line, not hang (the unbounded version is `[exit=124]`).

### Phase 6 — structs by value, and the layout query

**Files:** `src/schema.rs`, `src/marshal.rs`, `src/lib.rs`.

**Deliverable:** `{ struct: { x: "i32", y: "i32" } }` as parameter and as result; computed offsets
cross-checked against `middle::Type::structure(...).size()`; `layout(type)` exposing `{ size, align, offsets }`.

**Test:** the 8-byte register-class struct (`point_scale`) and the 24-byte MEMORY-class struct (`triple_sum`,
which exercises libffi's hidden `sret`) both round-trip; a struct object missing a field throws `"Schema"`
naming it; and `layout({ struct: { b: "i8", n: "i32" } })` is asserted directly to be
`{ size: 8, align: 4, offsets: { b: 0, n: 4 } }`.

---

## 7. Rejected alternatives

| Rejected | Why |
|---|---|
| **Building den:ffi this cycle** | Zero P0/P1 rows; gated on `den:permissions` (P0, does not exist); den:http is P0 with three P0 rows ([15 §3.18](15-stdlib-parity-gap.md):1616-1618). The P1 work that outranks it is the three open items in 15:3798's six-row embedding list — two `partial`s and one `missing`, the other three being `present`/`den_better` — plus the wasm `epoch_interruption` row at 15:3759 (fact 0). This is the primary recommendation, not a footnote. |
| **`libffi::high`** | Fixed arity `Closure0..12`, `CType` for primitives only, compile-time return type (`high/mod.rs:1-24`, `high/types.rs:43`). Cannot express a runtime symbol table. |
| **`libffi::low` directly** | Re-implements `middle`'s memory management for no gain, and its `call_return_small_big_endian_result` (`low.rs:471-492`) carries an empirical, not derived, struct rule (*"Testing has shown that these types appear at result"*). `middle` + `call_return_into` avoids it. |
| **Hand-rolled ABI classification** | This is precisely where FFI breaks silently rather than loudly, and precisely why Bun ships no struct-by-value. Deno maintains a libffi fork rather than reimplementing. |
| **A builder DSL (`fn([i32,i32], i32)`, `struct({x: f64})`)** | Twelve runtime exports whose only job is to be arguments; a `Cif` in a class field makes the schema `!Send` and therefore realm-local and non-serializable, needing a bespoke `DataCloneError` case. Its one real win — compile-time option-key checking (§3.4) — is not worth that. Its stated win, CIF-at-declaration, is not a win: both forms validate before any call. |
| **`libloading` 0.9** | Nicer (`Symbol<'lib, T>` ties the symbol to the library at compile time), but a new crate for a tie den cannot use anyway — the address must outlive the borrow to live in a class field, and `Symbol::into_raw` discards it. `dlopen2` is already installed (fact 9), and the runtime `live` flag is what actually converts use-after-dispose into a throw. |
| **`ptr.create(bigint)` / `ptr.offset(p, n)`** | They compile a forged arbitrary read against the very `.d.ts` that calls pointers unforgeable (§4.5). Deno has both; den does not. Removing them closes the forgery door only — `ptr.view` on a *live* pointer is still an arbitrary read, and §5.1 says so rather than letting the removal imply a bounds check. |
| **A length-less pointer view (`UnsafePointerView`)** | Structurally cannot bounds-check (`deno.d.ts:6536`). `view(p, byteLength)` with a mandatory length instead. |
| **Bun's `Pointer = number`** | Cannot address past 2^53 (`2**53+1 → 9007199254740992`), forgeable, arithmetic-able. |
| **Bun's `i64_fast` / `u64_fast`** | Silent precision loss. 64-bit is always `bigint`. |
| **`BigInt.asUintN(64, x) === x` as the u64 guard** | Broken in quickjs-ng for every width that is a multiple of 32 (fact 8). Comparison guards + decimal-string conversion instead. |
| **`ref()` / `unref()`** | ARCHITECTURE §7.5 rule 1 already makes an armed callback keep the loop alive via its `ctx.spawn`-ed pump, and dispose already takes it back. Deno needs `threadSafe()` purely as a refcount; den does not. |
| **An `in_sync_call` AtomicBool deadlock detector** | Racy (check-then-post) and would have to touch JS from the one thread that may not (§4.7). Replaced by a deterministic marshal-time refusal **plus** a bounded `blocking_recv`, which is what the flag was really groping for: the refusal cannot see a stored callback fired *into* a sync call, and the timeout is the only thing that breaks that cycle. |
| **`Box::leak` for "the closure must outlive C"** | Compiles, looks right, aborts at shutdown with a quickjs assertion that names nothing about FFI (fact 11). Context userdata instead — but note that the container is not free either: `Closure::new` borrows its userdata for `'a` (`middle/mod.rs:443`, `PhantomData<&'a ()>` at `:422-427`), so `CallbackEntry` costs one documented lifetime widening behind a `Pin<Box<Slot>>` and a load-bearing field order (§4.7). |
| **`tokio::task::spawn_local` / a `LocalSet`** | den is `#[tokio::main]` multi-thread with no `LocalSet`, so `spawn_local` panics; and a `LocalSet` task could only reach a `Ctx<'js>` through `async_with`, which parks on the mutex `idle()` holds (doc 09 fact 4). `ctx.spawn` gives everything a `LocalSet` would. |
| **In-process C compilation (`cc()` / TinyCC)** | A C toolchain inside the runtime for a P3 feature. den's portable-native path is WebAssembly ([15 §6](15-stdlib-parity-gap.md):3930). |
| **N-API / `.node` addons** | A whole second ABI. den's native plugins are compile-time Rust modules behind cargo features ([15 §6](15-stdlib-parity-gap.md):3933). |
| **`Cif::call<R>` anywhere in the marshaller** | `R` is not known at compile time, and a wrong `R` makes libffi write a full register into a smaller cell (`low.rs:415-423`) — the one genuine widening trap in the API. `call_return_into` corrects sub-register returns itself and writes exactly `type.size()` bytes (fact 3), so it is used exclusively. |
| **Caching a per-symbol argument-cell array** | The obvious profile-driven optimisation, and unsafe for any signature with a struct-by-value parameter: `low.rs:411-413` — *"libffi modifies some of the pointers in args if the struct is large enough ... this leads to an issue if args is being reused across multiple calls."* Marshal fresh per call (§4.3). |
| **Exposing dlopen flags to JS** | 15 §3.18 rates it P3. den pins `RTLD_NOW \| RTLD_LOCAL` via `open_with_flags` and does not surface the choice; non-breaking to add later because unknown schema keys throw (§4.3). |
| **`viewSource`, `linkSymbols`, variadics** | Debugging aid for Bun's JIT trampolines; raw-pointer symbol tables; and variadic calls need `ffi_prep_cif_var` plus a per-call CIF — none justified by a row. |

---

## 8. Open questions

1. **ABI verification beyond x86-64 SysV.** Every struct probe here ran on one target. AArch64 HFA/HVA,
   Windows x64 (structs > 8 bytes by hidden reference) and 32-bit are untested. The `open()`-time size
   cross-check catches gross layout disagreement; it does **not** catch a correct-size wrong-offset case.
   Either phase 6 gets an aarch64 CI leg or the claim is scoped in the docs. Unresolved.
2. **Who frees a returned `malloc`'d pointer?** Declaring the library's own `free` as a symbol is the answer,
   but it is currently implicit. It should be documented, and possibly the `.d.ts` should not offer
   `result: "pointer"` without a comment pointing at it.
3. **May a `Library`, `Pointer` or `Callback` cross a worker boundary?** They must not — `Cif` and `Closure`
   are `!Send`, and a `Pointer`'s provenance is realm-local. `postMessage` should throw `DataCloneError`
   explicitly rather than serialize to something useless. Not designed here.
4. **What the residual deadlock's timeout should be.** The question is no longer *"is a diagnostic worth it"*
   — §4.7 establishes the case is an **unbounded** deadlock (probe `[exit=124]`), not a stall bounded by the
   sync call, so the bounded `blocking_recv` is mandatory rather than optional. What is open is the number and
   its configurability: too short and a legitimately slow realm gets a zero handed to C, too long and the
   process looks hung. Candidates: a fixed conservative default (5-10 s) with no knob; a per-symbol
   `callbackTimeout` in the schema; or refusing callbacks outright on any library that also declares a sync
   symbol, which is airtight and probably too strict. Not decided.
5. **Does `layout()` need a `packed` option before v1?** Deferred deliberately, and non-breaking to add
   because unknown schema keys throw. But a script that needs it today has no workaround at all except
   inserting explicit padding fields.
6. **The `.d.ts` distribution story.** Do not write
   `types/den-ffi.d.ts` into the tree until `den:ffi` exists, and when it does, ship it through the same
   `den types` mechanism as the rest — one file, one `include_str!`, one CLI branch. Sequencing, not design.
7. **CLOSED — the `asUintN` bug has no den-side remediation.**
   `grep -rn 'asUintN' --exclude-dir=target --exclude-dir=.git --exclude-dir=.qwen .` over the working tree hits
   only this document and the vendored `vendor/test262/test/built-ins/BigInt/asUintN/` fixtures; den's own Rust
   and JS contain none. The "repo-wide" framing and its remediation task were a no-op. The engine bug itself is
   real and reproduced (fact 8, root cause pinned at `quickjs.c:57282` with `JS_LIMB_BITS 32` at `:442`); the
   only outstanding action is the **upstream quickjs-ng report**, and it is independent of whether den:ffi is
   ever built.
8. **The `.d.ts` is not yet the design's `.d.ts`.** Two edits are owed to `/tmp/denplan/G-ffi/den-ffi.d.ts`
   before anyone treats the §3.3 exit-0 run as certifying this document: add the `sigBrand` field to
   `Callback<P, R>` (§3.2 detail 2) with a tenth `@ts-expect-error` in `use.ts`, and delete the stale
   mapped-type comment at `:61-62` (§3.4). Both are verified fixes, neither has been folded back.

---

## Probe directories

| Dir | What |
|---|---|
| `/tmp/denplan/R7/` | libffi + rquickjs seam. `c/probe.c` + `c/libprobe.so` (the C shim: `add_i32`, `add_u64`, `add_f64`, `my_strlen`, `noop_void`, `point_scale`, `triple_sum`, `apply_cb`, `apply_cb_on_thread`, `register_cb`). `src/main.rs` = p1 (dynamic calls) … p6 (dangling closure → SIGSEGV). `src/bin/lifetime.rs` (E0515 borrow tie), `src/bin/unload.rs` (stale symbol after close → exit 139), `src/bin/panic_cb.rs` (non-unwinding panic → exit 134). The `negative.rs` `!Send` compile probe was not retained; its error text is quoted in fact 4. |
| `/tmp/denplan/R7/seam/` | `src/main.rs` cases a/b/c/d against a real rquickjs realm: a = foreign thread + `ctx.spawn` pump (exit 0), b = the sync-call deadlock (`timeout 12` → exit 124), c = same-thread `OwnedCtx` re-entry (exit 0), d = the same C function under `spawn_blocking` (exit 0). `src/bin/marshal.rs` = M1/M1b/M1c/M1d (BigInt range + the `asUintN` bug), M2 (exact u64 bits), M3 (`as_bytes` byteOffset + detached), M4 (`2**53+1`). |
| `/tmp/denplan/R8/` | Competitor surfaces. `probe.c` + `libprobe.so`; `deno.d.ts` (from `deno types`), `bun-ffi.d.ts`; `deno_probe.ts` (full pass), `deno_struct.ts` (unchecked struct buffer), `deno_ts2.ts`/`deno_ts3.ts` (threadSafe drop, close-inside-callback SIGSEGV), `deno_nb.ts` (GC pressure); `bun_probe.ts`/`bun_probe2.ts` (no structs, CString object, threadsafe crash), `bun_ts.ts`, `bun_cc.ts` + `hello.c` (TinyCC); `infer_deno.ts` (no `as const`), `tsprobe/infer_bun.ts`, `tsprobe/infer_bun2.ts` (the `as const` requirement). |
| `/tmp/denplan/G-ffi/` | **The shipped type layer.** `den-ffi.d.ts`, `use.ts` (exit 0, nine consumed `@ts-expect-error`), `flip.ts` (the nine diagnostics of §3.3), `tsconfig.json` (`strict` + `exactOptionalPropertyTypes` + `skipLibCheck: false`). Reproduce: `cd /tmp/denplan/G-ffi && npx -y -p typescript@5.9 tsc -p tsconfig.json`. |
| `/tmp/denplan/A-ffi/`, `/tmp/denplan/B-ffi/` | The two candidate type layers this design was grafted from: A = plain data (the base), B = schema-values-as-classes (source of the layout query and the callback-refusal rule). Retained for the contravariance note in §7 and the forged-pointer demonstration in §4.5. |
| `/tmp/denplan/V2/` | **The verification pass that rewrote facts 3 and 10, §3.2, §3.4, §4.5, §4.6, §4.7 and §8 q7.** `rust/` = the libffi and dlopen2 probes plus their C shim `rust/c/probe.c` (`main.rs` = the P2 return-cell canary refuting the widening claim; `bin/realm_struct.rs` = the `Closure<'static>` container that does not compile; `bin/residual.rs` + `fire_on_thread_and_join()` = the unbounded residual deadlock, `[exit=124]`; `bin/unload.rs` `[exit=139]`; `bin/{cb,send_probe,panic_cb,dangling}.rs`). `ts/` = the type-layer probes and the `noconst` / `notuple` / `mapped` / `brand` variants plus `stress.ts`. `bufs.js`, `bigint.js`, `disp.js`, `using.ts` = engine-side checks on the working-tree binary. **Reproduce:** `cd /tmp/denplan/V2/rust && cargo run --quiet [--bin cb\|send_probe\|realm_struct\|panic_cb\|dangling\|unload\|residual]` and `cd /tmp/denplan/V2/ts && npx -y -p typescript@5.9 tsc -p tsconfig[.flip\|.stress\|.brand\|.noconst\|.notuple\|.mapped].json`. |

Threading and event-loop constraints: [09](09-rquickjs-threads-and-event-loop.md).
Shutdown model: [16](16-cancellation-without-tokens.md), [17](17-graceful-shutdown-and-external-stop.md).
Requirements and priorities: [15 §3.18 / §3.19](15-stdlib-parity-gap.md).
