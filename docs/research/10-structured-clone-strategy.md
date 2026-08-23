# 10 — Structured clone for cross-runtime messaging

Status: research note for the Web Workers feature. Written against **quickjs-ng as vendored in
rquickjs-sys 0.12.2** (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-sys-0.12.2/quickjs/quickjs.c`,
`BC_VERSION 26`, quickjs.c:37656) and **rquickjs 0.12.2**. Every claim below is either a `file:line`
that was read or an empirical result from a probe that was compiled and run inside this workspace
(§0 records the probe method; the probe files were deleted afterwards — they are reproducible from
the snippets here).

**Bottom line:** `JS_WriteObject2` / `JS_ReadObject` carries roughly 85 % of the structured clone
algorithm, cross-runtime, including cycles, shared references and typed-array/buffer aliasing —
things a hand-rolled IR gets wrong on the first three tries. It cannot carry Errors, DataViews or
DOMExceptions, it accepts Symbols that the spec must reject, it reports every rejection as a
`TypeError` rather than a `DataCloneError`, and it has one latent stream-corruption bug (§4.4).
The recommendation is **byte serialiser + a small JS pre/post pass** (§7, §9), not a pure-Rust IR (§8).

---

## 0. How the empirical claims were obtained

A temporary integration test in `den-core/tests/` created **two independent `rquickjs::Runtime`s**,
evaluated a value in runtime A, serialised it with `JS_WriteObject2`, and read it back in runtime B
with `JS_ReadObject` — exactly the topology a worker will have (one OS thread, one QuickJS heap
each). The raw call shape used throughout (and reproduced in §6):

```rust
const WFLAGS: i32 = (qjs::JS_WRITE_OBJ_SAB | qjs::JS_WRITE_OBJ_REFERENCE) as i32;
const RFLAGS: i32 = (qjs::JS_READ_OBJ_SAB  | qjs::JS_READ_OBJ_REFERENCE)  as i32;

let mut len: qjs::size_t = 0;
let mut sab = qjs::JSSABTab { tab: std::ptr::null_mut(), len: 0 };
let buf = unsafe { qjs::JS_WriteObject2(ctx.as_raw().as_ptr(), &mut len, value.as_raw(), WFLAGS, &mut sab) };
```

Results are quoted inline as `probe:` lines. They are reproducible; nothing below is from memory.

---

## 1. The complete writer tag list, checked against the HTML spec table

The tag enum is `quickjs.c:37631-37653`; the writer dispatch is `JS_WriteObjectRec`
(quickjs.c:38218-38380); the reader dispatch is `JS_ReadObjectRec` (quickjs.c:39512-39647).

| BC tag | writer site | HTML spec (structured-data.html) | verdict |
|---|---|---|---|
| `BC_TAG_NULL` / `UNDEFINED` / `BOOL_FALSE` / `BOOL_TRUE` / `INT32` / `FLOAT64` / `STRING` | 38230-38256 | step 4 "primitive" | ✅ |
| `BC_TAG_BIG_INT` | `JS_WriteBigInt` 37888 | step 4 (BigInt primitive) | ✅ |
| `BC_TAG_OBJECT_VALUE` | 38327-38333, classes `NUMBER`/`STRING`/`BOOLEAN`/`BIG_INT` | steps 7-10 boxed primitives | ✅ |
| `BC_TAG_DATE` | 38322-38326 | step 11 | ✅ |
| `BC_TAG_REGEXP` | 38318-38321, `JS_WriteRegExp` 38207 | step 12 (source + flags; `lastIndex` intentionally not carried) | ✅ |
| `BC_TAG_ARRAY_BUFFER` | `JS_WriteArrayBuffer` 38174 | step 13.2 — **copies** the bytes (`dbuf_put(&s->dbuf, abuf->data, abuf->byte_length)` 38183) and carries `max_byte_length`, so resizable ABs survive | ✅ |
| `BC_TAG_SHARED_ARRAY_BUFFER` | `JS_WriteSharedArrayBuffer` 38189 | step 13.1 — writes the **pointer** (`bc_put_u64(s, (uintptr_t)abuf->data)` 38197) | ✅ but see §3 |
| `BC_TAG_TYPED_ARRAY` | `JS_WriteTypedArray` 38160 | step 14 typed arrays — writes class id, length, offset, then recurses into the buffer, so **two views over one buffer stay aliased** | ✅ |
| `BC_TAG_ARRAY` | `JS_WriteArray` 38080 | step 18 | ⚠️ holes (see §1.3) |
| `BC_TAG_OBJECT` | `JS_WriteObjectTag` 38123 | step 24 plain object | ⚠️ accessors (see §1.3) |
| `BC_TAG_MAP` / `BC_TAG_SET` | `js_map_write` 52802 | steps 15-16 | ⚠️ zombie bug (§4.4) |
| `BC_TAG_OBJECT_REFERENCE` | 38286-38295 | step 25 memory / cycles | ✅ |
| `BC_TAG_SYMBOL` | 38361-38372 | **step 5: must throw `DataCloneError`** | ❌ **over-permissive** |
| `BC_TAG_FUNCTION_BYTECODE` / `BC_TAG_MODULE` / `BC_TAG_TEMPLATE_OBJECT` | 37941 / 38035 / 38091 | not clonable | ✅ gated behind `JS_WRITE_OBJ_BYTECODE`, which den must never set |

`probe: primitives+wrappers OK -> {"a":1,"b":"héllo😀","c":[1,2,3],"d":0,"e":"ab+c/gi/lastIndex=0",
"f":"bigint10","g":"[object Number]3","h":"[object String]s","i":"[object Boolean]true",
"j":"[object BigInt]5","k":null,"l":true,"m":"NaN","n":true,"o":1.5,"p":1099511627776,
"proto":true,"ctorRealm":true}` — note `n:true` is `Object.is(v, -0)`, so **negative zero survives**,
and `proto:true` confirms the rebuilt object is wired to the *receiver's* `Object.prototype`
(`JS_ReadObjectTag` calls plain `JS_NewObject`, quickjs.c:39230), which is what cross-realm
structured clone requires.

`probe: typed arrays share buffer OK -> {"aIsU8":true,"shared":true,"b":[1027,1541],"off":2,
"bytes":[1,2,3,4,5,6,7,8],"clamped":true,"f64":[1.5],"big":"1","f16":1.5}` — `shared:true` is
`v.a.buffer === v.buf && v.b.buffer === v.buf`. Buffer aliasing across two views **is** preserved,
including `Uint8ClampedArray`, `BigInt64Array` and `Float16Array`. This is the single hardest thing
to get right by hand and it is free here.

`probe: resizable AB OK -> {"len":4,"resizable":true,"max":8}` — `maxByteLength` survives
(quickjs.c:38181 writes it, 39365 reads it), matching spec step 13.2.5.

### 1.1 What is MISSING (required by the spec, refused by the writer)

| Missing | Spec | Observed |
|---|---|---|
| **Error objects** | step 17 | `probe: Error WRITE FAILED -> TypeError: unsupported object class`. `JS_CLASS_ERROR` (quickjs.c:132) has **no case** in the writer's class switch (38304-38351), so it lands in `default:` → `JS_ThrowTypeError(s->ctx, "unsupported object class")` (38346). Same for a nested Error: `probe: Error nested WRITE FAILED -> TypeError: unsupported object class`. |
| **DataView** | step 14.5 | `probe: DataView WRITE FAILED -> TypeError: unsupported object class`. `is_typed_array` (quickjs.c:58395) deliberately excludes `JS_CLASS_DATAVIEW`. |
| **DOMException** | it is a platform object with `[Serializable]` | `probe: DOMException instance WRITE FAILED -> TypeError: unsupported object class` — `JS_CLASS_DOM_EXCEPTION` (quickjs.c:193) is likewise absent from the switch. |
| **Boxed Symbol** | step 22 (must throw) | `probe: boxed symbol WRITE FAILED -> TypeError: unsupported object class` — refused, but with the wrong error type. |

Map and Set are **not** missing in this version (`BC_TAG_MAP`/`BC_TAG_SET` at 38334-38341, added by
quickjs-ng). RegExp is **not** missing. Those two "likely candidates" from older QuickJS forks are
resolved here.

### 1.2 What it accepts that structured clone must REJECT

* **Symbols, both as values and as property keys.** `probe: symbol value OK ->
  {"s":"symbol","desc":"x","g":true,"w":true}` — a `Symbol("x")` round-trips as a live symbol,
  a registered `Symbol.for("reg")` comes back `=== Symbol.for("reg")` on the other side, and
  `Symbol.iterator` comes back as the receiver's own well-known symbol. Spec step 5 says
  `DataCloneError`. `probe: symbol key OK -> {"syms":1,"a":2}` — a symbol-keyed property is
  carried too, whereas `EnumerableOwnProperties` (step 26.4) yields String keys only. **Must be
  pre-screened.**
* Functions are *not* silently dropped: with `JS_WRITE_OBJ_BYTECODE` unset, `JS_TAG_OBJECT` with
  a function class hits the same `default:` arm. `probe: function WRITE FAILED -> TypeError:
  unsupported object class`; `probe: arrow fn top WRITE FAILED -> TypeError: unsupported object
  class`. Correct outcome, wrong error type. Setting `JS_WRITE_OBJ_BYTECODE` would turn a function
  into serialisable bytecode — **den must never set that flag** on a message path.

### 1.3 Behaviours that differ from the spec but are *observably* fine

* **Array holes are filled.** `probe: array holes OK -> {"len":3,"hole":true}` — `hole` is
  `1 in v`, so `[1,,3]` comes back as `[1,undefined,3]`. `JS_WriteArray` (38080) iterates
  `0..len` with `JS_GetPropertyUint32`, which materialises holes as `undefined`. Spec step 26.4
  uses `EnumerableOwnProperties`, so a hole should stay a hole. Divergence, low severity, and
  fixing it costs a full property-walk — recommend accepting it and documenting.
* **Non-index array properties are dropped.** `probe: array extra prop + holes -> {"len":3,
  "hole":false,"foo":undefined}` (`foo` absent from the JSON). Spec step 26.4 would carry `foo`.
  Same divergence class, same recommendation.
* **Prototypes are flattened to `Object.prototype`; non-enumerable and private fields are
  dropped.** `probe: class instance + private + nonenum + frozen OK -> {"keys":["q"],
  "proto":true,"frozen":false,"fz":1}` — a class instance loses its prototype and its `#p`, a
  non-enumerable own property is dropped (`JS_WriteObjectTag` filters on `JS_PROP_ENUMERABLE`,
  38139), and `Object.freeze` does not survive. **All of this is spec-correct** (step 24 keeps
  only own enumerable properties and the result is an ordinary object).
* **Getters throw rather than being invoked.** `probe: accessor WRITE FAILED -> TypeError: only
  value properties are supported` — `JS_WriteObjectTag` rejects `JS_PROP_TMASK` (38141-38143).
  Spec step 26.4.1.1 says the getter **is** invoked. This is a real incompatibility and is one of
  the two reasons the supplement in §7 has to walk plain objects itself.

---

## 2. Cycles and shared references — `JS_WRITE_OBJ_REFERENCE`

With the flag, the writer keeps a `JSObjectList` and emits `BC_TAG_OBJECT_REFERENCE <idx>` for any
object already seen (quickjs.c:38286-38295). Without it, it sets `p->tmp_mark` and throws
(38297-38302). The reader registers every object it builds via `BC_add_object_ref` **before**
recursing into its contents (e.g. `JS_ReadObjectTag` at 39231, `JS_ReadArray` at 39264), and
resolves a reference with `js_dup` of the recorded `JSObject*` (39606-39620), so **identity is
genuinely shared, not duplicated**.

`probe: cycle+shared ref OK -> {"cycle":true,"shared":true,"arrShared":true,"arrCycle":true}` —
self-referencing object, one object reachable by two paths coming back `===`, an array containing
itself. `probe: cycle without REFERENCE flag -> Some("TypeError: circular reference")` confirms the
flag is mandatory.

`probe: map+set+identity-in-map OK -> {"mapSize":2,"same":true,"setHas":true,"setObjSame":true}` —
identity is preserved *through* Map values and Set members, i.e. the object graph is global, not
per-container. `JS_ReadTypedArray` even reserves its object-table slot *before* reading the buffer
and back-patches it (39310-39313, 39344-39346) so that a typed array referenced from elsewhere in
the graph resolves correctly.

**Reader must pass `JS_READ_OBJ_REFERENCE` or the tag is a hard error**:
`"object references are not allowed"` (39609).

---

## 3. SharedArrayBuffer via `JS_WRITE_OBJ_SAB` — and why den should skip it in v1

The writer emits the raw backing pointer and appends it to `s->sab_tab` (38189-38205); the reader
takes the pointer straight out of the stream and constructs a SAB over it
(`JS_ReadSharedArrayBuffer`, 39392-39441). The contract is therefore:

1. The runtime must have SAB hooks installed via `JS_SetSharedArrayBufferFunctions`
   (binding at `rquickjs-sys .../x86_64-unknown-linux-gnu.rs:1471`). The reader refuses the tag
   unless `ctx->rt->sab_funcs.sab_dup` is non-null (quickjs.c:39589-39592).
2. Those hooks must allocate a **refcounted, process-global** block. quickjs-libc's reference
   implementation puts an atomic refcount in a header immediately before the data
   (`JSSABHeader` / `js_sab_alloc` / `js_sab_free` / `js_sab_dup`, quickjs-libc.c:3934-3971) and
   installs them at quickjs-libc.c:4658-4666.
3. The **sender** must `sab_dup` every pointer in the returned `sab_tab` before handing the message
   off (quickjs-libc.c:4271-4274) and the message-free path must `sab_free` each one
   (`js_free_message`, quickjs-libc.c:3997-4007). The receiver's `js_array_buffer_constructor3`
   dups again on its own behalf (quickjs.c:57783-57786) and the SAB finalizer frees through
   `rt->sab_funcs.sab_free` (quickjs.c:57932-57934).

`probe: SAB with flag (no sab hooks) -> (sab_tab.len = 1) READ FAILED -> SyntaxError: invalid tag
(tag=16 pos=7)` — the write succeeds and populates `sab_tab`, but the read fails because rquickjs
never installs SAB hooks. `probe: SAB without flag -> Some("InternalError: unsupported tag (-1)")`.

**Recommendation: no SAB in v1.** rquickjs exposes no safe wrapper for the hooks, `den-stdlib-wasm`
deliberately ships no `SharedArrayBuffer` (`SUPPORTS_SHARED_MEMORY` in `backend.rs` is den's own
limit, per ARCHITECTURE.md §5.1), and the hooks are process-global runtime state that would have to
be installed on *every* worker runtime with a matching allocator, plus a `sab_dup`/`sab_free`
discipline that is a use-after-free waiting to happen if a message is dropped on a terminate path.
Pass `JS_WRITE_OBJ_SAB` **off**, and let the pre-pass reject `SharedArrayBuffer` with a proper
`DataCloneError` (spec step 13.1.1 permits refusal when not cross-origin isolated; refusing
unconditionally is a defensible superset). Turning it on later is additive and does not change the
wire format for anything else.

---

## 4. ArrayBuffer transfer

### 4.1 The serialiser always copies

`JS_WriteArrayBuffer` does `dbuf_put(&s->dbuf, abuf->data, abuf->byte_length)` (quickjs.c:38183)
and `JS_ReadArrayBuffer` constructs with `alloc_flag = true` over `s->ptr`, commented
`// makes a copy of the input` (quickjs.c:39374-39381). There is **no** hook to move a backing store
into or out of the stream: the buffer is `js_malloc`'d from the *writer's* runtime allocator, and
with `rust-alloc` on (den's feature set, per root `Cargo.toml`) that is the Rust global allocator
per runtime via `RustAllocator` (`rquickjs-core/src/runtime/raw.rs:140,166-174`).

### 4.2 So transfer is copy-then-detach

Serialise first, detach after the write succeeds — which is exactly the spec's order
(`StructuredSerializeWithTransfer` validates the list in step 2, serialises in step 3, and performs
`DetachArrayBuffer` in step 5.4.3, *after* serialisation succeeded).

rquickjs already wraps the detach: `ArrayBuffer::detach(&mut self)` →
`qjs::JS_DetachArrayBuffer` (`rquickjs-core/src/value/array_buffer.rs:259-261`). The underlying
`JS_DetachArrayBuffer` (quickjs.c:58030-58056) runs the buffer's `free_func`, nulls `data`, sets
`byte_length = 0`, marks `detached`, and walks `abuf->array_list` zeroing every view's
`count`/`ptr` — so views are neutered too, no dangling.

Probe of the exact sequence (write → detach sender → read receiver):

```
sender after detach: {"len":0,"detached":true,"viewLen":0}
receiver: {"bytes":[9,8,7,6],"viewShares":true}
```

The receiver got the bytes *and* the view still aliases the buffer; the sender is fully neutered.

### 4.3 Zero-copy is not achievable, and does not matter

A true move would need `JS_WriteObject` to emit a pointer for a non-shared AB (it does not — only
the SAB path does, 38197) and the reader to adopt it with a matching free func across two different
`js_malloc` arenas. Not expressible through the public API. **Spec-observable behaviour of
copy-then-detach is identical** — the sender is detached, the receiver has the bytes; only the cost
differs, and it is one `memcpy` per transferred buffer. **Recommend copy-then-detach for v1**, and
note that a later optimisation exists outside the byte stream: strip transferred buffers from the
graph before serialising and ship them as raw `Vec<u8>` alongside the blob (rebuilt receiver-side
with `ArrayBuffer::new`, which takes ownership of a `Vec` — array_buffer.rs:91-119). That is a
genuine zero-copy path if it ever matters; it is not worth the complexity in v1.

Pre-validation the spec demands and the serialiser gives for free: a detached buffer anywhere in the
graph throws. `probe: detached AB WRITE FAILED -> TypeError: ArrayBuffer is detached`
(`JS_WriteArrayBuffer` 38177-38180 → `JS_ThrowTypeErrorDetachedArrayBuffer`, message at 57961-57964).
Spec step 13.2.1 wants `DataCloneError`, so this too needs re-tagging (§5).

### 4.4 ⚠️ A real writer bug: zombie Map/Set records desync the stream

`js_map_write` (quickjs.c:52802-52822) writes `map_state->record_count` as the entry count and then
iterates **every** record in `map_state->records` — including "zombie" records, i.e. deleted entries
kept alive because a live iterator still holds a reference (`map_delete_record`, quickjs.c:52273-52294,
keeps `mr->empty = true` with `key`/`value` set to `JS_UNDEFINED` when `ref_count` has not hit 0).
Zombies are excluded from `record_count` but **not** from the write loop, so the writer emits more
key/value pairs than it announced. The reader (`js_map_read`, 52761) consumes exactly the announced
count; the surplus bytes are then misparsed as whatever follows in the stream.

Characterised empirically (expected `entries a/c/d` and `sentinel:"S"` in all five cases):

```
no iterator:                                {"entries":[["a",1],["c",3],["d",4]],"sentinel":"S"}
iterator parked before deleted key:         {"entries":[["a",1],["c",3],["d",4]],"sentinel":"S"}
iterator parked past deleted key:           {"entries":[["a",1],[null,null],["c",3]],"sentinel":"LOST"}
forEach reentrant delete:                   {"entries":[["a",1],["c",3],["d",4]],"sentinel":"S"}
delete during iteration, captured mid-loop: {"entries":[["a",1],["c",3],["d",4]],"sentinel":"S"}
```

The third line is silent corruption: a bogus `[undefined, undefined]` entry appears, `"d"` is lost,
and the *sibling property* `sentinel` of the enclosing object is eaten by the desynchronised
stream. Trigger: a Map (or Set) that has a **live iterator parked past a key that was then
deleted**, serialised while that iterator is still reachable.

**This is the single strongest argument for the hybrid design.** The §7 pre-pass rebuilds every Map
and Set into a fresh one before serialisation; a freshly constructed Map has no zombie records, so
den never hands a vulnerable Map to the writer. Confirmed: with the pre-pass in place,
`probe2: zombie map record OK -> {"size":1,"entries":[[1,1]],"z":5}` and the sibling property
survives. Worth reporting upstream regardless.

### 4.5 Three transfer-list cases the serialiser will NOT catch for you

`JS_DetachArrayBuffer` (quickjs.c:58030-58056) guards only `!abuf || abuf->detached`. It has **no**
immutable check and **no** detach-key concept, and it unconditionally calls `abuf->free_func`. So
`Message::serialize` must refuse these itself, *before* step 4 of §9 detaches anything:

| Transfer-list entry | Spec | Why den must check it by hand | How |
|---|---|---|---|
| `WebAssembly.Memory#buffer` | `StructuredSerializeWithTransfer` step 2: `[[ArrayBufferDetachKey]]` not `undefined` → `DataCloneError` | den-stdlib-wasm builds it with `JS_NewArrayBuffer(..., free_func = None)` over the wasm store's pages and rebuilds the detach key by shadowing `transfer`/`transferToFixedLength`/`transferToImmutable`/`resize` with throwing **own** properties (`memory.rs:260-283` `alias`, `:288-326` `seal_against_transfer`). A Rust-level `ArrayBuffer::detach` bypasses that JS-side seal and yanks the buffer out from under the `LiveBuffer` registry. | Treat "has an own `transfer` property" as "has a detach key" (the seal makes it non-configurable, so script cannot remove it; a user who *adds* one to a plain buffer merely opts out of transfer). Better long-term: den-stdlib-wasm exposes the marker as one shared symbol in `den-utils`; not needed for v1. |
| immutable `ArrayBuffer` (`buf.transferToImmutable()`, quickjs-ng supports it: `abuf->immutable` quickjs.c:865, getter `immutable` :58370) | not transferable → `DataCloneError` | `JS_DetachArrayBuffer` would happily detach it. | `qjs::JS_IsImmutableArrayBuffer(val)` (quickjs.c:58057, binding :1384) → `DataCloneError`. |
| already-detached `ArrayBuffer` | step 2 → `DataCloneError` | See §9 step 1: rquickjs's `ArrayBuffer::from_value` / `as_raw` both go through `JS_GetArrayBuffer`, which **throws a pending `TypeError`** on a detached buffer (quickjs.c:58097-58099) while returning null. A bare `is_none()` check leaves that exception armed and it surfaces at the next unrelated call. | Test with `qjs::JS_IsArrayBuffer(val)` (binding :1381) for the type, then read the JS `detached` getter (`Object::get::<_, bool>("detached")`, quickjs.c:58369) — no pending exception either way. |

Also note the writer does not carry the immutable bit (`JS_WriteArrayBuffer` writes `byte_length`,
`max_byte_length`, bytes — quickjs.c:38182-38186), so **an immutable buffer clones as a mutable
one**. Third accepted divergence for v1 (see §12); if it ever matters, `prepare` tags it and `restore`
calls `transferToImmutable()` on the copy.

### 4.6 `MessagePort` goes through the same pipeline, as a placeholder

The scope includes `MessageChannel`, so `worker.postMessage({ port: ch.port2 }, [ch.port2])` is the
canonical use of the transfer list, not a corner case. `MessagePort` is `[Transferable]`, **not**
`[Serializable]` (HTML §9.4.4): a port in the graph that is *not* in the transfer list is a
`DataCloneError` (step 20, platform object without serialization steps); one that *is* gets replaced
by a placeholder recording its index in the transfer list, and the receiver's `MessageEvent.ports`
is the frozen array of the re-materialised ports, in transfer-list order. The spec also rejects a
port listed twice and a port whose `[[Detached]]` is already true (already shipped).

This fits the §7 tags with no new mechanism: `prepare(value, transferPorts)` gets the list, and `copy`
emits `tag("Port", { index })` for a port it finds in it (identity via the same `seen` memo, so a
port reachable twice is one placeholder) and `fail("MessagePort")` for any other port. `restore(graph,
ports)` swaps placeholders for `ports[index]`. Detecting a port is the §9 `kindOf` trick again:
compare `JS_GetClassID` against the class id of the native `MessagePort` class (or `as_class::<…>()`
if it is an `#[rquickjs::class]`; 09 §10 says this is the host-class special case). The Rust side
then moves the port's channel halves out of the sender's object and ships them in `Message.ports`
alongside the bytes — the port object on the sender is left `[[Detached]]` so a second transfer fails.
The `Message` struct in §9 is widened accordingly.

---

## 5. Does rquickjs 0.12 wrap any of this? — No.

`grep -rn "JS_WriteObject\|JS_ReadObject" rquickjs-core-0.12.2/src` returns exactly two hits, both
for **module bytecode**, not values:

* `value/module.rs:341` — `Module::load` calls `JS_ReadObject(..., JS_READ_OBJ_BYTECODE | JS_READ_OBJ_ROM_DATA)`.
* `value/module.rs:473` — `Module::write` calls `JS_WriteObject(...)` and frees with
  `qjs::js_free(ctx.as_ptr(), buf as _)` after copying into a `Vec` (module.rs:480-487).

There is no `write_object` / `read_object` on `Value`. The raw bindings exist and are re-exported
through `rquickjs::qjs`: `JS_WriteObject`, `JS_WriteObject2`, `JS_ReadObject`, `JS_ReadObject2` and
`struct JSSABTab { tab: *mut *mut u8, len: size_t }` at
`rquickjs-sys-0.12.2/src/bindings/x86_64-unknown-linux-gnu.rs:1671-1717`.

`Module::write` is the pattern to copy for the ownership obligations. The exact sequence, with all
of them:

```rust
/// Serialise `value` out of its runtime. Returns owned bytes with no tie to `ctx`.
fn write_bytes(ctx: &Ctx<'_>, value: &Value<'_>) -> Result<Vec<u8>> {
  let mut len: qjs::size_t = 0;
  // No SAB (§3), no BYTECODE (never — it would serialise functions), REFERENCE for cycles (§2).
  const FLAGS: i32 = qjs::JS_WRITE_OBJ_REFERENCE as i32;
  // psab_tab = null: quickjs then `js_free`s its own sab_tab internally (quickjs.c:38466).
  let buf = unsafe { qjs::JS_WriteObject2(ctx.as_raw().as_ptr(), &mut len, value.as_raw(), FLAGS, ptr::null_mut()) };
  if buf.is_null() {
    // The writer left a pending exception on `ctx`. rquickjs's convention is to return
    // `Error::Exception` and let the caller `ctx.catch()` it (there is no `From<Value> for Error`).
    return Err(Error::Exception);
  }
  let out = unsafe { slice::from_raw_parts(buf, len as usize) }.to_vec();
  // MUST free with the *writer runtime's* allocator, not Rust's.
  unsafe { qjs::js_free(ctx.as_raw().as_ptr(), buf.cast()) };
  Ok(out)
}

/// Deserialise into a different runtime.
fn read_bytes<'js>(ctx: &Ctx<'js>, bytes: &[u8]) -> Result<Value<'js>> {
  const FLAGS: i32 = qjs::JS_READ_OBJ_REFERENCE as i32;
  let raw = unsafe { qjs::JS_ReadObject(ctx.as_raw().as_ptr(), bytes.as_ptr(), bytes.len() as _, FLAGS) };
  // `Ctx::handle_exception` is `pub(crate)` (rquickjs-core/src/result.rs:724) — NOT callable from den.
  // Test the tag by hand; the pending exception stays on `ctx` for the caller to `catch()`.
  if unsafe { qjs::JS_IsException(raw) } {
    return Err(Error::Exception);
  }
  Ok(unsafe { Value::from_raw(ctx.clone(), raw) })
}
```

(`den-stdlib-wasm/src/memory.rs:267-278` already uses exactly this `JS_IsException` → `Error::Exception`
→ `Value::from_raw` shape for a raw `JS_NewArrayBuffer` call — copy it.)

Obligations, each verified:

* `Value::as_raw()` (`rquickjs-core/src/value.rs:427`) is a **borrow** — it does not dup, and
  `JS_WriteObject2` does not consume. Do **not** `JS_FreeValue` it; the `Value` still owns it.
* `Value::from_raw` (`value.rs:438`) is `unsafe` and **takes ownership** of the `JSValue`. Use it
  exactly once on the reader result; never on `as_raw()` output.
* The write buffer must be freed with `js_free(ctx, ...)` (bindings line 437) because it came from
  that context's allocator — with `rust-alloc` that is `RustAllocator`, and freeing it with Rust's
  `dealloc` directly would be a layout mismatch.
* If `psab_tab` is non-null the caller owns `sab_tab.tab` and must `js_free` it too
  (quickjs.c:38461-38467). Passing null makes quickjs free it — simplest, and correct when SAB is
  off.
* Byte buffers are `Vec<u8>`: `Send`, no lifetime tie to either runtime. This is what makes the
  message crossable between OS threads at all.

`ctx.catch()` (`context/ctx.rs:257-262`) retrieves the pending exception; `Exception::throw_*`
helpers are at `value/exception.rs:105-195`.

---

## 6. DataCloneError: den does not have to build a DOMException — quickjs-ng already has one

This was the surprise of the investigation. quickjs-ng ships a **native `DOMException` class**
(`JS_CLASS_DOM_EXCEPTION`, quickjs.c:193; implementation 62138-62360) with the full legacy
name→code table including `{ "DataCloneError", "DATA_CLONE_ERR" }` (quickjs.c:62174), a prototype
that inherits from `Error` (`JS_NewObjectClass(ctx, JS_CLASS_ERROR)`, 62332), a `code` getter, and
`[Symbol.toStringTag]`.

It is registered by `JS_AddIntrinsicAToB` (quickjs.c:63339-63344), which calls
`JS_AddIntrinsicDOMException` when the class is not yet registered — and `JS_NewContext` calls
`JS_AddIntrinsicAToB` unconditionally (quickjs.c:2550). den uses `AsyncContext::full`
(`den-core/src/engine.rs:234`), which routes to `JS_NewContext`
(`rquickjs-core/src/context/async.rs:163`). Therefore **`DOMException` is already a global in every
den context today**, confirmed twice:

* In-process: `probe: DOMException global exists OK -> {"t":"function","here":"function","code":25,
  "isErr":true,"str":"DataCloneError: m"}`.
* Through the actual den binary (`target/debug/den`):
  `name, DataCloneError, code, 25, instanceof Error, true, toString, DataCloneError: m, has stack, string`.

Throwing one from Rust needs no JS-land class-building at all — the `den-stdlib-wasm/src/error.rs`
pattern (a `DEFINE_ERRORS` snippet evaluated at module-evaluate time and cached in userdata,
error.rs:22-104) is **not needed here**. There is a direct binding:

```rust
// rquickjs-sys .../x86_64-unknown-linux-gnu.rs:886
pub fn JS_ThrowDOMException(ctx: *mut JSContext, name: *const c_char, fmt: *const c_char, ...) -> JSValue;

/// Throw `DOMException(message, "DataCloneError")`. Always returns the Err payload to propagate.
pub fn throw_data_clone(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
  let message = CString::new(message).unwrap_or_default();
  unsafe {
    qjs::JS_ThrowDOMException(ctx.as_raw().as_ptr(), c"DataCloneError".as_ptr(), c"%s".as_ptr(), message.as_ptr());
  }
  rquickjs::Error::Exception
}
```

`probe: JS_ThrowDOMException -> DataCloneError: Symbol() could not be cloned.` — verbatim.
(Note `JS_ThrowDOMException` `vsnprintf`s into a 256-byte stack buffer, quickjs.c:62305 — always
pass `c"%s"` as the format and the real text as an argument; never pass user text as the format
string. It also `assert`s that `JS_CLASS_DOM_EXCEPTION` is registered, quickjs.c:62307 — an abort,
not an exception, in a context built with `AsyncContext::custom`/`builder` that skipped the AToB
intrinsic. Worker contexts must be built with `AsyncContext::full`, as `Engine::new` does.)

For the JS-side pre-pass the constructor is simply reachable as `new DOMException(msg, "DataCloneError")`.

### 6.1 Required `DataCloneError` cases and how each is produced

| Case | Spec | Writer's own behaviour | Plan |
|---|---|---|---|
| Symbol (value) | step 5 | **accepts** (§1.2) | pre-screen → `DataCloneError` |
| Symbol (key) | step 26.4 | **accepts** | pre-pass skips it (`Object.keys` is String-only) |
| Function / class / arrow | step 21 | `TypeError: unsupported object class` | pre-screen → `DataCloneError` |
| Proxy | step 23 | `TypeError: unsupported object class` (`probe`) | pre-screen with `JS_IsProxy` → `DataCloneError` **before touching it**, so no trap runs |
| Promise | step 22 | `TypeError: unsupported object class` | writer rejects; re-tag its error → `DataCloneError` |
| WeakMap / WeakSet / WeakRef | step 22 | `TypeError: unsupported object class` (all three probed) | as Promise |
| boxed Symbol | step 22 | `TypeError: unsupported object class` | as Promise |
| generator / `arguments` object | step 22/23 | `TypeError: unsupported object class` (both probed) | as Promise |
| detached ArrayBuffer | step 13.2.1 | `TypeError: ArrayBuffer is detached` | as Promise |
| SharedArrayBuffer | step 13.1.1 (not cross-origin isolated) | accepted with the flag; unreadable without hooks | flag off → writer's `InternalError`; re-tag |
| platform object without `[Serializable]` | step 20 | `TypeError: unsupported object class` | as Promise |

The last column collapses to **two mechanisms**: (a) a handful of explicit pre-screens for the cases
the writer would wrongly *accept* or where a Proxy trap must not run, and (b) a blanket "any error
escaping the serialiser becomes a `DataCloneError`" re-tag. The re-tag is honest because, once the
pre-pass has run, **every** remaining writer failure is a non-serialisable-value failure.

---

## 7. The minimal supplement, written and proven end-to-end

The supplement is one JS function pair — `prepare` on the sender, `restore` on the receiver —
built once per context from a source string, in the `den-stdlib-wasm/src/error.rs` style (a
`ctx.eval` of a factory at module-evaluate time, error.rs:91-104), plus one tiny Rust callback
`kindOf` that answers the three questions JS cannot: *is it a Proxy*, *is it an Error*, *is it a
plain object*. Those map to `JS_IsProxy` (bindings:1066), `JS_IsError` (bindings:810, backed by
`JS_GetClassID(val) == JS_CLASS_ERROR`, quickjs.c:11604-11607) and a `JS_GetClassID` comparison
against the class id of a freshly made `{}`. `instanceof` cannot be used for these: it is
forgeable, and `Object.getPrototypeOf(proxy)` invokes a trap. (The `JS_CLASS_*` ids are a private
enum in quickjs.c, not in quickjs.h, so rquickjs-sys has **no** constants for them — the
"compare against a freshly made instance" trick is the only way, and it extends to `DOMException`
and `MessagePort`. `JS_IsError` is `JS_CLASS_ERROR == JS_GetClassID(val)`, quickjs.c:11604-11607,
so it is **false** for a `DOMException` instance, whose class is `JS_CLASS_DOM_EXCEPTION`; the sketch
below therefore checks `DOMException` separately, before the Error arm.)

`prepare(value) -> [graph, tagged]` does an **identity-preserving shallow rebuild**: it walks the
graph with a `Map` memo (so cycles and shared references survive into the rebuilt graph, and the
serialiser's own `JS_WRITE_OBJ_REFERENCE` then encodes them), passes the natively-supported leaf
objects through *by reference* (ArrayBuffer, typed arrays, Date, RegExp, boxed primitives), rebuilds
Array/Map/Set/plain-object containers (which is what invokes getters, skips symbol keys, and
sidesteps §4.4), throws `DataCloneError` on Symbol / function / Proxy, and replaces the three
missing types with tagged plain objects. `restore` reverses it in place. The `tagged` flag lets the
receiver skip the whole restore walk when nothing was tagged — the common case.

The tag key is `"\u0000den:structured-clone"`. A leading NUL cannot appear in a JS identifier and
essentially never in real data; a user object that carries it verbatim is not misinterpreted anyway
because `restore` only revives objects whose tag value is one of the three known kinds
(`probe2: user object carrying the tag key OK -> {"isErr":false, ...}` — the spoof round-trips as a
plain object). Note when embedding in Rust: write it as the JS escape `\u0000`, not a literal NUL —
`ctx.eval` goes through `CString` and a real NUL byte fails with `Error::InvalidString`.

Error mapping follows spec step 17 exactly: name is coerced to one of the seven
`Error, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError` and anything else
becomes `"Error"` (step 17.2); `message` is carried only when it is an **own** property (step 17.3);
`stack` is carried as an implementation-defined string (step 17.5). `cause` is carried in addition —
the spec does not require it, and carrying it costs one recursive `copy()` call, so it is worth it.

Measured behaviour of the complete pipeline (`prepare` → `JS_WriteObject2` → cross-runtime →
`JS_ReadObject` → `restore`):

```
zombie map record:      OK (tagged=false) -> {"size":1,"entries":[[1,1]],"z":5}
plain:                  OK (tagged=false) -> {... ,"date":true,"u8":true}
error top:              OK (tagged=true)  -> {"isTypeError":true,"msg":"boom","stackIsString":true,
                                              "cause":"inner","ownMsg":["message","stack","cause"]}
error shared + cycle:   OK (tagged=true)  -> {"same":true,"selfCause":true,"deep":true,
                                              "myName":"Error","myIsError":true,
                                              "myProtoIsError":true,"aggName":"Error"}
DOMException:           OK (tagged=true)  -> {"name":"NotFoundError","msg":"msg","code":8,"isDom":true}
DataView shares buffer: OK (tagged=true)  -> {"isDV":true,"off":2,"len":4,"first":42,"shares":true}
getters + symbol keys:  OK (tagged=false) -> {"p":{"x":1},"pProto":true,"g":{"z":3,"w":4},"syms":0,
                                              "gHasOwnZ":3}
symbol value:           PREPARE -> DataCloneError: Symbol(x) could not be cloned.
function:               PREPARE -> DataCloneError: function f could not be cloned.
proxy:                  PREPARE -> DataCloneError: #<Proxy> could not be cloned.
promise / weakmap / boxed symbol: WRITE -> TypeError: unsupported object class   (needs re-tag)
detached AB:            WRITE -> TypeError: ArrayBuffer is detached              (needs re-tag)
detached DataView:      PREPARE -> TypeError: ArrayBuffer is detached or resized (needs re-tag)
map/set with error keys+cycle: OK (tagged=true) -> {"keyIsErr":true,"same":true,"selfMap":true,
                                                    "setHasErr":true,"setHasMap":true}
```

Highlights worth calling out: an Error shared by three paths comes back as **one** object with a
self-referential `cause` (`same/selfCause/deep` all true) — the tagged replacement participates in
the serialiser's reference table, so identity survives the substitution. A subclassed `MyErr`
correctly degrades to `Error` per step 17.2. A `DataView` comes back aliasing the *same*
reconstructed `ArrayBuffer` as its sibling `Uint8Array` (`shares:true`), because the tagged object
carries the buffer by reference and `restore` rebuilds the view over the already-deserialised
buffer. Getters are invoked (`gHasOwnZ:3` is a data property on the far side), symbol keys are
dropped (`syms:0`), class instances flatten to `Object.prototype` (`pProto:true`).

Sketch (verbatim from the working probe, trimmed):

```js
(kindOf) => {                                  // kindOf: 0 plain, 1 Proxy, 2 Error, 3 other
  const TAG = "\u0000den:structured-clone";
  const ERROR_NAMES = new Set(["Error","EvalError","RangeError","ReferenceError","SyntaxError","TypeError","URIError"]);
  const fail = (what) => { throw new DOMException(`${what} could not be cloned.`, "DataCloneError"); };
  const hasOwn = Object.hasOwn;                // own-ness check for `message`/`cause` (spec step 17.3)
  // Handled natively by the byte serialiser: pass through by reference so identity survives.
  const isLeaf = (v) => (ArrayBuffer.isView(v) && !(v instanceof DataView)) || v instanceof ArrayBuffer
    || v instanceof Date || v instanceof RegExp || v instanceof Number || v instanceof String
    || v instanceof Boolean || v instanceof BigInt;

  const prepare = (value) => {
    const seen = new Map();
    let tagged = false;
    const tag = (kind, fields) => { tagged = true; return { [TAG]: kind, ...fields }; };
    const copy = (v) => {
      switch (typeof v) {
        case "symbol":   fail(String(v));
        case "function": fail(`function ${v.name || "(anonymous)"}`);
        case "object":   if (v !== null) break;
        default:         return v;
      }
      const hit = seen.get(v);
      if (hit !== undefined) return hit;               // cycles + shared refs
      const kind = kindOf(v);
      if (kind === 1) fail("#<Proxy>");                // before any trap can run
      if (isLeaf(v)) { seen.set(v, v); return v; }
      let out;
      if (v instanceof DOMException) { out = tag("DOMException", { name: v.name, message: v.message }); seen.set(v, out); }
      else if (kind === 2) {                            // Error — spec step 17
        out = tag("Error", {
          name:    ERROR_NAMES.has(v.name) ? v.name : "Error",
          message: hasOwn(v, "message") ? String(v.message) : undefined,
          stack:   typeof v.stack === "string" ? v.stack : undefined,
        });
        seen.set(v, out);
        if (hasOwn(v, "cause")) out.cause = copy(v.cause);
      }
      else if (v instanceof DataView) { out = tag("DataView", { buffer: v.buffer, byteOffset: v.byteOffset, byteLength: v.byteLength }); seen.set(v, out); }
      else if (Array.isArray(v))  { out = new Array(v.length); seen.set(v, out); for (const k of Object.keys(v)) out[k] = copy(v[k]); }
      else if (v instanceof Map)  { out = new Map(); seen.set(v, out); for (const [k, val] of v) out.set(copy(k), copy(val)); }
      else if (v instanceof Set)  { out = new Set(); seen.set(v, out); for (const k of v) out.add(copy(k)); }
      else if (kind === 0)        { out = {}; seen.set(v, out); for (const k of Object.keys(v)) out[k] = copy(v[k]); }
      else { seen.set(v, v); out = v; }                // platform/exotic: let the writer reject it
      return out;
    };
    return [copy(value), tagged];
  };
  // restore(): same walk on the receiver, in place (the graph is freshly deserialised and ours),
  // reviving TAG'd objects into Error / DOMException / DataView. Full source in the probe.
  return { prepare, restore };
}
```

---

## 8. The pure-Rust alternative, and why not

An `enum StructuredValue` IR with its own object table, built and consumed through rquickjs's typed
APIs (`Object::own_props`, `Array`, `TypedArray`, `ArrayBuffer::as_bytes`, …).

| | byte serialiser + supplement (§7) | pure-Rust IR |
|---|---|---|
| **Correctness vs spec** | ~85 % free and *battle-tested by the engine's own test suite*; the remaining 15 % is the §7 walk. Known divergences: array holes filled, non-index array props dropped (§1.3). | Every rule is den's to get right. The two that bite: **buffer aliasing** (two views over one AB must stay aliased — the serialiser does this at 38160-38172; an IR needs a buffer identity table) and **identity through Map keys** (a shared object used as a Map key must stay one object). Both are silent-wrongness bugs, not crashes. |
| **Amount of code** | ~150 lines of JS + ~80 lines of Rust (raw call + DataCloneError + wiring). | Realistically 700-1200 lines: an enum with ~20 variants, a serialiser, a deserialiser, an object table on both sides, per-typed-array-kind handling (11 kinds), BigInt (rquickjs `BigInt` has no limb accessor — would have to go through a decimal string), boxed primitives, RegExp flags reconstruction. |
| **Risk / unsafe surface** | Four `unsafe` calls, all in one ~30-line function, all with the ownership rules written down in §5 and mirrored from `Module::write`. Plus the §4.4 engine bug, which the pre-pass already sidesteps. | Almost no `unsafe` — genuinely its best property. But risk ≠ unsafe: 1000 lines of subtle graph-walking logic has more *defects* than 30 lines of audited FFI. |
| **Performance** | One `memcpy`-heavy pass in C, one `Vec<u8>` allocation, one pass back. Buffers copied once. | One JS→Rust conversion per value (each `Object::get` crosses the FFI boundary and refcounts), an intermediate allocation per node, then the reverse. Comfortably slower, and the intermediate `StructuredValue` tree is pure overhead. |
| **Unit-testable in isolation** | Only via a real `Runtime` — but that is *already* how every den test works (`den-core/tests/webassembly.rs`, `den-stdlib-wasm/src/error.rs:139-208`), and two runtimes in one test is three lines. | Genuinely nicer: `StructuredValue` can be built, asserted and fuzzed with no engine. Real advantage, and the only one. |
| **Bonus** | The blob is `Vec<u8>`, `Send`, with no runtime tie — exactly the message payload the thread-per-worker design needs, with no extra step. | The IR must itself be `Send`, which means no `Value<'js>` anywhere in it — achievable but another constraint to enforce. |

**Recommendation: the byte serialiser plus the §7 supplement.** The IR wins on testability alone,
and loses on every other axis, most decisively on the one that matters: the serialiser already
solves buffer aliasing and cross-container identity, which is precisely where a hand-written clone
is wrong for months before anyone notices. The testability gap is closed cheaply by testing the
public API through two real runtimes, which is what the tests in §10 do.

---

## 9. Recommended public API

Lives in a new `den-stdlib-worker` crate (or `den-core/src/structured_clone.rs` if `structuredClone()`
should land before workers do — it is independently useful and is the natural first PR).

```rust
/// A structured-clone-serialised value. Owns its bytes; no tie to any runtime.
/// `Send` is the whole point: this is what crosses the channel to a worker thread.
pub struct Message {
  bytes:  Vec<u8>,
  tagged: bool,   // whether `restore` must run on the far side (skips a walk in the common case)
  ports:  Vec<PortHalves>,  // §4.6: the channel ends moved out of each transferred MessagePort,
                            // in transfer-list order; `tagged` is forced true when non-empty
}

static_assertions::assert_impl_all!(Message: Send);

impl Message {
  /// Serialise `value` out of `ctx`, transferring each ArrayBuffer / MessagePort in `transfer`.
  ///
  /// Order matches StructuredSerializeWithTransfer: the transfer list is validated (step 2:
  /// duplicates, non-transferables, detached / immutable / detach-keyed buffers, shipped ports
  /// all rejected — §4.5, §4.6), the value is serialised (step 3), and only then are the
  /// transferred buffers detached and the ports moved (step 5.4.3) — so a failed clone leaves the
  /// sender's buffers and ports intact.
  ///
  /// Every failure is a `DOMException` with `name === "DataCloneError"`.
  pub fn serialize(ctx: &Ctx<'_>, value: Value<'_>, transfer: &[Value<'_>]) -> Result<Self>;

  /// Rebuild the value inside `ctx` — which may be (and normally is) a different runtime
  /// on a different OS thread. The second element is what becomes `MessageEvent.ports`
  /// (frozen array, transfer-list order; empty for `structuredClone()`).
  pub fn deserialize(self, ctx: &Ctx<'js>) -> Result<(Value<'js>, Vec<MessagePort<'js>>)>;
}
```

Note `Value<'js>` is neither `Send` nor valid in another context (09 §1.3-1.4): `serialize` must
run under the *sender's* `Ctx` and `deserialize` under the *receiver's* — i.e. inside the two
different `AsyncContext::with` / `async_with!` closures on the two threads. Only `Message` crosses.

Internals, in order:

1. **Validate the transfer list** (spec step 2). Each entry must be an `ArrayBuffer`
   (`qjs::JS_IsArrayBuffer`, binding :1381) or a `MessagePort` (§4.6). For a buffer: not
   detached (read the JS `detached` getter — **do not** use `ArrayBuffer::from_value` /
   `as_raw().is_none()` for this: both call `JS_GetArrayBuffer`, which returns null **and throws a
   pending `TypeError`** for a detached buffer, quickjs.c:58097-58099, array_buffer.rs:308-320),
   not immutable (`qjs::JS_IsImmutableArrayBuffer`, binding :1384), and without a detach key
   (§4.5 — `WebAssembly.Memory#buffer`). Duplicates rejected by pointer identity
   (`Value: PartialEq` compares tag+bits, value.rs:44-54). Any violation →
   `throw_data_clone`. **Note the intentional v1 divergence:** the spec ignores transferred
   objects during serialisation and moves them, whereas den serialises them normally (a copy)
   and then detaches — observably identical, per §4.3.
2. **`prepare`** (§7) → `(graph, tagged)`; propagates its own `DataCloneError`s. Takes the
   transfer list's ports so it can emit `Port` placeholders (§4.6).
3. **`JS_WriteObject2`** with `JS_WRITE_OBJ_REFERENCE` only (§5). On failure, catch the pending
   exception and **re-throw it as a `DataCloneError`**, preserving the original message as context
   (`"could not be cloned: unsupported object class"`). This is the blanket re-tag from §6.1.
4. **Detach** each transferred buffer with `ArrayBuffer::detach` (§4.2) and **move** each
   transferred port's channel halves into `Message.ports` (§4.6). After the write, never before.
5. `deserialize`: `JS_ReadObject` with `JS_READ_OBJ_REFERENCE`, then `restore` if `tagged`
   (always when ports were transferred). A read failure is **not** a `DataCloneError`; it is
   reported to the caller as an ordinary `Err`, and the *transport* turns it into a `messageerror`
   event at the target (HTML §9.4.4 step for `StructuredDeserializeWithTransfer` failure; 08 §2.6,
   11 test I-15). Two legitimate causes exist besides den bugs and the §4.4 desync: the receiver's
   `set_memory_limit` being hit while the reader allocates (quickjs-ng throws out-of-memory, not a
   panic), and `JS_ThrowStackOverflow` from the reader's own depth check (quickjs.c:39518).

`structuredClone(value, { transfer })` is then two lines: `Message::serialize(ctx, value, &transfer)?
.deserialize(ctx)`, same context in and out. It is worth shipping first precisely because it
exercises the entire path with no threading involved.

The `prepare`/`restore` pair is built once per context and cached in **context userdata**, exactly
as `WebAssemblyErrors` does (`den-stdlib-wasm/src/error.rs:97-104` stores, :112-115 retrieves) — so
it survives a script deleting globals, and costs one `ctx.eval` per context.

---

## 10. The unit tests that prove it

All of them are `#[test]` in `den-core/tests/structured_clone.rs`, using **two `Runtime`s** unless
noted (same-runtime tests are only for `structuredClone()` semantics). Naming follows den's
convention of naming the scenario and outcome (`den-stdlib-wasm/src/error.rs:143`).

**Per type in the spec table — round-trip preserves value and class:**

1. `primitives_round_trip_including_negative_zero_and_nan`
2. `strings_round_trip_including_astral_and_lone_surrogates`
3. `bigint_round_trips_across_the_limb_boundary` (0, ±1, `2n**63n`, `-(2n**64n)-1n`)
4. `boxed_number_string_boolean_bigint_stay_boxed`
5. `date_preserves_time_value_including_invalid_date`
6. `regexp_preserves_source_and_flags_and_resets_last_index`
7. `array_buffer_round_trips_and_is_a_copy_not_an_alias`
8. `resizable_array_buffer_preserves_max_byte_length`
9. `every_typed_array_kind_round_trips` (all 11 + `Uint8ClampedArray`, table-driven)
10. `typed_array_preserves_byte_offset_and_length_over_a_larger_buffer`
11. `two_views_over_one_buffer_still_share_it_after_cloning` ← the aliasing invariant
12. `data_view_round_trips_and_shares_its_buffer_with_a_sibling_view` ← exercises the supplement
13. `map_round_trips_preserving_insertion_order`
14. `set_round_trips_preserving_insertion_order`
15. `array_round_trips_including_nested_arrays`
16. `plain_object_round_trips_and_gets_the_receivers_object_prototype`
17. `class_instance_is_flattened_to_a_plain_object`
18. `getters_are_invoked_and_become_data_properties`
19. `non_enumerable_and_private_fields_are_dropped`
20. `error_round_trips_with_name_message_and_stack`
21. `error_subclass_name_degrades_to_error` (spec step 17.2)
22. `error_without_own_message_gets_no_message`
23. `error_cause_round_trips`
24. `dom_exception_round_trips_preserving_name_and_code`

**Graph shape:**

25. `self_referential_object_round_trips_as_a_cycle`
26. `object_reachable_by_two_paths_stays_one_object`
27. `array_containing_itself_round_trips`
28. `shared_object_used_as_a_map_key_and_value_stays_one_object`
29. `error_shared_by_three_paths_stays_one_error` ← proves tagged objects join the reference table
30. `deeply_nested_graph_does_not_overflow_the_stack` (~10k deep; the writer checks
    `js_check_stack_overflow`, quickjs.c:38224, so this must surface as a clean error, not a crash)

**Transfer:**

31. `transferred_array_buffer_arrives_with_its_bytes`
32. `transferred_array_buffer_is_detached_in_the_sender`
33. `views_onto_a_transferred_buffer_are_neutered_in_the_sender`
34. `transfer_list_with_a_duplicate_throws_data_clone_error`
35. `transfer_list_containing_a_non_array_buffer_throws_data_clone_error`
36. `transfer_list_containing_an_already_detached_buffer_throws_data_clone_error`
37. `a_failed_clone_leaves_transferred_buffers_undetached` ← proves the step 3-before-step 5 order

**Every `DataCloneError` case (assert `name === "DataCloneError"`, not merely that it threw):**

38. `symbol_value_throws_data_clone_error`
39. `symbol_key_is_silently_dropped_not_thrown` (spec: keys are String-only, so no throw)
40. `function_throws_data_clone_error`
41. `class_constructor_throws_data_clone_error`
42. `proxy_throws_data_clone_error_without_invoking_a_trap` ← trap that would `throw`/increment
43. `promise_throws_data_clone_error`
44. `weak_map_throws_data_clone_error`
45. `weak_set_throws_data_clone_error`
46. `weak_ref_throws_data_clone_error`
47. `boxed_symbol_throws_data_clone_error`
48. `detached_array_buffer_in_the_graph_throws_data_clone_error`
49. `shared_array_buffer_throws_data_clone_error` (v1 policy, §3)
50. `a_den_platform_object_throws_data_clone_error` (e.g. a `den:sqlite` handle)
51. `generator_object_throws_data_clone_error`
52. `arguments_object_throws_data_clone_error`
53. `data_clone_error_is_an_instance_of_dom_exception_and_error`

**Regression guards for the two engine hazards:**

54. `map_with_a_live_iterator_parked_past_a_deleted_key_round_trips_intact` ← §4.4; asserts both the
    entries **and** a sibling property of the enclosing object survive
55. `set_with_a_live_iterator_parked_past_a_deleted_key_round_trips_intact`
56. `bytecode_is_never_accepted_on_read` — feed a `Module::write` blob to `deserialize` and assert
    it fails (`"no bytecode allowed"`, quickjs.c:39572). Guards against anyone adding
    `JS_READ_OBJ_BYTECODE` and turning a message channel into arbitrary-code execution.
57. `a_user_object_carrying_the_internal_tag_key_is_not_revived` ← anti-spoofing

**Transfer-list cases the engine does not guard (§4.5, §4.6):**

58. `transfer_list_containing_an_immutable_buffer_throws_data_clone_error` (`buf.transferToImmutable()` first)
59. `immutable_buffer_clones_as_a_mutable_copy` ← documents divergence #3
60. `transfer_list_containing_a_wasm_memory_buffer_throws_data_clone_error_and_leaves_it_attached`
    (`cfg(feature = "stdlib-wasm")`; asserts `memory.buffer.byteLength` unchanged afterwards)
61. `rejecting_a_detached_buffer_leaves_no_pending_exception` ← `ctx.has_exception()` is false after
    the `DataCloneError`; guards against the `JS_GetArrayBuffer` trap
62. `message_port_in_the_transfer_list_arrives_in_event_ports_in_order` (two ports, reversed order)
63. `message_port_in_the_graph_but_not_in_the_transfer_list_throws_data_clone_error`
64. `message_port_listed_twice_throws_data_clone_error`
65. `already_shipped_message_port_throws_data_clone_error`
66. `a_port_reachable_by_two_paths_is_one_port_on_the_receiver`

**Fuzz target** (`cargo-fuzz`, per the testing pyramid in CLAUDE.md): feed arbitrary bytes to
`Message::deserialize` and assert it never panics or aborts — only returns `Err`. The reader is a
parser over attacker-shaped input the moment anything but den writes those bytes, and it has a
checksum (`bc_csum`, quickjs.c:37806) that will reject most mutations but not all.

---

## 11. Prior art

* **quickjs-ng's own `os.Worker`** (`quickjs-libc.c`). `js_worker_postMessage` (4226-4293) writes
  with `JS_WRITE_OBJ_SAB | JS_WRITE_OBJ_REFERENCE`, **memcpys the buffer out of the runtime
  allocator into `malloc`** (4252-4257, comment: *"must reallocate because the allocator may be
  different"* — the same reason den must copy into a `Vec`), dups every SAB pointer, and pushes onto
  a mutex-guarded intrusive list with a waker. `handle_posted_message` (2675-2712) reads with
  `JS_READ_OBJ_SAB | JS_READ_OBJ_REFERENCE` and builds `{ data }` by hand. No Error handling, no
  transfer list, no `DataCloneError` — it is a proof of concept, not a spec implementation, but it
  is proof the byte format crosses runtimes.
* **txiki.js** (`src/mod_channel.c`, read via GitHub; `src/worker.c` contains only lifecycle). Same
  approach, one flag more: `JS_WRITE_OBJ_SAB | JS_WRITE_OBJ_REFERENCE | JS_WRITE_OBJ_STRIP_SOURCE`,
  read back with `JS_READ_OBJ_SAB | JS_READ_OBJ_REFERENCE`. It also memcpys out of the QuickJS
  allocator into its own message struct for the same reason, refcounts SABs with `tjs__sab_dup` /
  `tjs__sab_free` and re-dups per subscriber on BroadcastChannel fan-out, and moves bytes through a
  refcounted lock-protected mailbox woken by `uv_async_send`. Two design points worth stealing:
  clone failures on *receive* are turned into a `messageerror`-kind delivery rather than being
  allowed to escape; and worker uncaught errors are flattened **by hand** to `{ message, name,
  stack }` before being sent, precisely because the serialiser cannot carry an Error — independent
  confirmation of §1.1. It builds `MessageEvent` in JS from `{ data, ports, kind }`; the C layer is
  transport plus (de)serialisation only. den should split the same way.

`JS_WRITE_OBJ_STRIP_SOURCE` (quickjs.h:1218) only affects function bytecode, which den never
writes, so it is a no-op here — skip it.

---

## 12. Decisions, in one place

1. Use `JS_WriteObject2` / `JS_ReadObject` for the heavy lifting. **Flags: `JS_WRITE_OBJ_REFERENCE`
   and `JS_READ_OBJ_REFERENCE`, nothing else.** Never `BYTECODE`. Not `SAB` in v1.
2. Supplement with one JS `prepare`/`restore` pair (§7) covering Error, DOMException, DataView, the
   `DataCloneError` pre-screens, getter invocation, symbol-key dropping, and the §4.4 workaround.
3. `DOMException` is already a global in every den context (§6) — no class needs building. Throw
   from Rust with `JS_ThrowDOMException(ctx, c"DataCloneError", c"%s", msg)`.
4. Transfer is copy-then-detach, detach strictly after a successful write (§4).
5. Ship `structuredClone()` first, in `den-core`; the worker channel then just moves `Message`
   values between threads.
6. Accept and document three divergences: array holes are filled, non-index array properties
   dropped, immutable `ArrayBuffer`s clone as mutable (§4.5).
7. File the `js_map_write` zombie-record bug upstream (§4.4).
8. The transfer list accepts `ArrayBuffer` **and** `MessagePort`; ports travel as `Port`
   placeholders plus `Message.ports` (§4.6). Transfer validation refuses detached, immutable and
   detach-keyed (`WebAssembly.Memory#buffer`) buffers by hand, because `JS_DetachArrayBuffer`
   checks none of them (§4.5).
9. Never use `ArrayBuffer::from_value` / `as_raw` as a detachment probe — they arm a pending
   `TypeError` (§4.5, §9 step 1). Never call `Ctx::handle_exception` — it is `pub(crate)` (§5).
10. Deserialisation failure is a `messageerror` delivery, not a `DataCloneError` and not a panic
    (§9 step 5).

---

## Verification log

Second pass (2026-08-22) against the same sources: `rquickjs-sys-0.12.2/quickjs/quickjs.c`,
`quickjs.h`, `src/bindings/x86_64-unknown-linux-gnu.rs`, `rquickjs-core-0.12.2/src/**`, and den's
`den-stdlib-wasm/src/memory.rs`, `den-core/src/engine.rs`. Every line number cited in the body was
re-read; each claim below is marked confirmed, corrected, or added.

| # | Claim | Result |
|---|---|---|
| 1 | `BC_VERSION 26` at quickjs.c:37656; writer dispatch 38218-38380; reader dispatch 39512+ | **confirmed** |
| 2 | `JS_CLASS_ERROR` (:132) has no arm in the writer's class switch → `"unsupported object class"` (default arm, :38344-38347); Map/Set **are** supported (`JS_WriteMap`/`JS_WriteSet` :52834-52842 → `js_map_write` :52802) | **confirmed** |
| 3 | §4.4 zombie-record desync: `js_map_write` writes `record_count` then `list_for_each` over **all** `records` with no `mr->empty` test; `map_delete_record` (:52273-52294) keeps `empty=true` zombies with `key`/`value = JS_UNDEFINED` while `ref_count > 0` and decrements `record_count`; `js_map_read` (:52761) loops exactly `prop_count` times | **confirmed** — real bug |
| 4 | `JS_WriteArrayBuffer` copies bytes and carries `max_byte_length` (:38182-38186); `JS_ReadArrayBuffer` constructs with `alloc_flag = true` ("makes a copy of the input", :39374-39381) | **confirmed** |
| 5 | `JS_ReadTypedArray` reserves slot `idx` before reading the buffer and back-patches `s->objects[idx]` (:39322-39324, :39343-39345) | **confirmed** |
| 6 | Symbols are written (`BC_TAG_SYMBOL`, :38361-38372) — only `JS_ATOM_TYPE_SYMBOL`/`GLOBAL_SYMBOL`; other atom types throw `"unsupported symbol type"` | **confirmed** (minor detail added here) |
| 7 | `psab_tab == NULL` → quickjs `js_free`s `sab_tab` itself (:38461-38465); on failure `tab = NULL, len = 0` (:38477-38480) | **confirmed** |
| 8 | Reader refuses SAB tag unless `allow_sab && ctx->rt->sab_funcs.sab_dup` (:39589-39592); `"object references are not allowed"` (:39609); `"no bytecode allowed"` (:39572) | **confirmed** |
| 9 | Both writer and reader call `js_check_stack_overflow` (:38222, :39518), so a ~10k-deep graph is a clean `InternalError`, not a crash | **confirmed** |
| 10 | `bc_csum` (:37806); the reader skips the checksum when the stored value is `UINT32_MAX` (:39668, "escape hatch") — a fuzz target should mutate that field too | **confirmed**, note added |
| 11 | `DOMException` is native: `JS_CLASS_DOM_EXCEPTION` (:193), `{ "DataCloneError", "DATA_CLONE_ERR" }` (:62175), `JS_AddIntrinsicDOMException` (:62329), registered from `JS_AddIntrinsicAToB` (:63338-63344), which `JS_NewContext` calls unconditionally (:2550); `AsyncContext::full` → `JS_NewContext` (async.rs:163); den uses `full` (engine.rs:234) | **confirmed** |
| 12 | `JS_ThrowDOMException(ctx, name, fmt, ...)` (:62300, binding :886) with a 256-byte `vsnprintf` buffer (:62309) | **confirmed**; **added**: it `assert`s the class is registered (:62307) — abort in a `custom` context |
| 13 | `Value::as_raw` borrows (value.rs:427), `Value::from_raw` is `unsafe` and takes ownership (:438); `Value: PartialEq` compares tag + bits (:44-54) | **confirmed** |
| 14 | `ctx.handle_exception(raw)?` in the `read_bytes` snippet | **corrected** — `Ctx::handle_exception` is `pub(crate)` (result.rs:724); snippet now tests `qjs::JS_IsException` and returns `Error::Exception` (the shape `memory.rs:267-278` already uses) |
| 15 | `Err(ctx.catch().into())` in the `write_bytes` snippet | **corrected** — no `From<Value> for Error` exists (result.rs:63-141; only `NulError`, io, etc.); return `Error::Exception` and let the caller `catch()` |
| 16 | `ctx.catch()` (ctx.rs:257-262) and `Exception::throw_*` helpers (exception.rs:105-195) | **confirmed** |
| 17 | `ctx.eval` goes through `CString::new` → `Error::InvalidString` on an embedded NUL (ctx.rs:141-149 `eval_raw`; result.rs:502) | **confirmed** |
| 18 | `ArrayBuffer::new` takes ownership of a `Vec` (array_buffer.rs:91-119, `drop_raw` rebuilds it); `ArrayBuffer::detach` → `JS_DetachArrayBuffer` (:259-261); `from_value` (:276) | **confirmed**, one line off on `from_value` |
| 19 | "`as_raw().is_none()` detects a detached buffer because `JS_GetArrayBuffer` returns null" | **corrected** — true but hazardous: `JS_GetArrayBuffer` (:58092-58105) calls `JS_ThrowTypeErrorDetachedArrayBuffer` before returning null, and rquickjs's `get_raw` (array_buffer.rs:308-320) does not clear it, so `from_value`/`as_raw`/`from_object` on a detached buffer all leave a pending exception. Replaced with `JS_IsArrayBuffer` + the `detached` getter (§4.5, §9 step 1) |
| 20 | `JS_DetachArrayBuffer` (:58030-58056) runs `free_func`, nulls `data`, zeroes every non-DataView view's `count`/`ptr` | **confirmed**; **added**: it has no immutable check and no detach-key concept |
| 21 | Zero-length `ArrayBuffer` has non-null `data` (`js_mallocz(ctx, max_int(len, 1))`, :57776) | **added** — so `from_value(new ArrayBuffer(0))` is `Some`, the empty-buffer transfer test will not false-fail |
| 22 | `JS_IsError` = `JS_CLASS_ERROR == JS_GetClassID(val)` (:11604-11607); `JS_IsProxy` (:51812, binding :1066); `JS_GetClassID` binding :776; **no** `JS_CLASS_*` constants in the bindings (the enum lives in quickjs.c) | **confirmed**; consequence for DOMException/MessagePort detection added to §7 |
| 23 | `JS_WRITE_OBJ_STRIP_SOURCE` is `1 << 4` at quickjs.h:1218 and only affects `allow_source` in function bytecode | **confirmed** |
| 24 | `Module::write`/`Module::load` are the only in-tree users of `JS_WriteObject`/`JS_ReadObject` (module.rs:341, :473, `js_free` :486) | **confirmed** |
| 25 | rust-alloc → `RustAllocator` (raw.rs:140-141) | **confirmed** |
| 26 | `JSSABTab { tab, len }` binding :1671; `JS_WriteObject2`/`JS_ReadObject2` :1691/:1708; `js_free` :437; `JS_SetSharedArrayBufferFunctions` :1471 | **confirmed** |
| 27 | quickjs-ng immutable ArrayBuffers: `abuf->immutable` (:865), `JS_IsImmutableArrayBuffer` (:58057, binding :1384), `transferToImmutable` (:58375); the writer does not carry the bit | **added** (§4.5, divergence #3) |
| 28 | den-stdlib-wasm's `Memory#buffer` is a `JS_NewArrayBuffer(..., free_func = None)` alias over the wasm pages (memory.rs:260-283) whose detach key is rebuilt as shadowing own properties (`:288-326`) — a Rust-level `detach` bypasses it | **added** (§4.5) |
| 29 | `MessagePort` transfer (ports placeholder + `Message.ports`, `MessageEvent.ports`, `DataCloneError` for un-transferred ports) — required by the scope, absent from the note; 11 §11 defers it here and 09 §10 names it a host-class special case | **added** (§4.6, §9) |
| 30 | Deserialisation failure handling: spec says `messageerror`, not a `DataCloneError`; OOM under `set_memory_limit` and the reader's stack check are legitimate causes | **added** (§9 step 5) |
| 31 | `hasOwn` was used but never defined in the §7 sketch | **corrected** |

Not re-verified here (out of this note's scope, covered by 09/11): tokio `block_in_place` on a
1-worker runtime, `AsyncRuntime::idle()` semantics, interrupt-handler polling sites.
