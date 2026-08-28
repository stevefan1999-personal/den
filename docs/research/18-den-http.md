# `den:http` — an HTTP server for a one-realm-per-thread runtime, and why the whole thing is one `ctx.spawn`

Status: research, 2026-08-28. Snapshot of the den working tree (branch `master`, at `34f1af3`, 222 uncommitted
paths), rquickjs-core 0.12.2 (`full-async, rust-alloc, parallel`), hyper 1.11.0, hyper-util 0.1.20, http 1.5.0,
http-body-util 0.1.2, h2 0.4.18, bytes 1.12.1, matchit 0.8.4, rustls 0.23.43 / rustls-webpki 0.103.15 /
ring 0.17.14 / tokio-rustls 0.26.1 / rustls-pki-types 1.15.1, native-tls 0.2.18, tokio-tungstenite 0.26.2 /
tungstenite 0.26.2, socket2 0.6.5, cookie 0.18.1, tokio 1.53.1, tokio-util 0.7.19, wasmtime 48.0.0,
rustc 1.98.0, Deno 2.9.4, Bun 1.3.9, Node v26.5.0, Linux x86_64.
**Not a living document.** Every claim carries a `file:line` into the working tree or a vendored source, or a line
quoted verbatim from a probe run. Nothing is from memory. For the current truth read
[ARCHITECTURE.md](../../ARCHITECTURE.md) or the code.

Builds on three settled documents and does not re-derive them:
[09](09-rquickjs-threads-and-event-loop.md) (what may cross a thread, what `idle()` does),
[16](16-cancellation-without-tokens.md) (no engine token; resources own their close handles),
[17](17-graceful-shutdown-and-external-stop.md) (drain is a resource method, and §7 q7 records that den has no
`serve()` and no listener `close()` — *this document is the answer to that open question*).
The requirements list is [15](15-stdlib-parity-gap.md) §3.6 (86 rows: 3 P0, 23 P1, 30 P2, 24 P3 + 6 not-applicable)
and its §4 cross-cutting themes. The standing rules are [14](14-runtime-feature-roadmap.md): no built-in installed
by evaluating JS/TS bootstrap source, cargo-feature-gated, and unsupported behaviour must fail explicitly.

Product stance, restated because it decides several rows below: **no Node/Deno/Bun compatibility aliases.** Take the
best capability, give it a smaller coherent API under `den:*`, implement it in Rust. Be better, not compatible.

## Sources read

| What | Path |
|---|---|
| den (working tree) | `den-core/src/engine.rs`, `src/{app,main}.rs`, `den-stdlib-whatwg-fetch/src/{lib,headers,request,body,fetch_op}.rs`, `den-stdlib-whatwg/src/{streams,local_http,websocket,urlpattern}.rs`, `den-stdlib-networking/src/{socket,tls,websocket,socket_addr}.rs`, `den-stdlib-worker/src/{abort,port}.rs`, `den-stdlib-fs/src/lib.rs`, `ARCHITECTURE.md` §3 §7.3 §7.5 |
| rquickjs-core 0.12.2 | `$R/rquickjs-core-0.12.2/src/` — `context/ctx.rs`, `runtime/{async,schedular,opaque}.rs` |
| hyper 1.11.0 | `$R/hyper-1.11.0/src/` — `server/conn/{http1,http2}.rs`, `service/{service,http}.rs`, `rt/{mod,bounds}.rs`, `body/{mod,incoming}.rs` |
| hyper-util 0.1.20 | `$R/hyper-util-0.1.20/src/` — `server/conn/auto/mod.rs`, `server/graceful.rs`, `rt/tokio.rs` |
| matchit 0.8.4 | `$R/matchit-0.8.4/src/` — `lib.rs`, `error.rs`, `router.rs`; `Cargo.toml` |
| rustls 0.23.43 / tokio-rustls 0.26.1 / rustls-pki-types 1.15.1 / native-tls 0.2.18 | same registry root |
| tungstenite / tokio-tungstenite 0.26.2 | `$R/tungstenite-0.26.2/src/handshake/mod.rs`, `$R/tokio-tungstenite-0.26.2/src/lib.rs` |
| Deno 2.9.4 | `deno types` output (`/tmp/denplan/R2/deno.d.ts`) + `ext/http/{00_serve.ts,http_next.rs,service.rs}` |
| Bun 1.3.9 | `bun-types@1.3.14` `serve.d.ts`; `bun-docs/runtime/http/routing.mdx` |
| Node v26.5.0 | `node:http` / `node:http2` docs |
| Probes | `/tmp/denplan/{writer,r1-realm-threading,R2,R3,R4,R5,R5b,r6}/` — table at the end |

`$R` = `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f`.

---

## 0. TL;DR — the facts an implementer must not get wrong

1. **`Ctx::spawn` has no `Send` bound, and that is the entire design.** `ctx.rs:418-423` is
   `pub fn spawn<F>(&self, future: F) where F: Future<Output = ()> + 'js` — no `Send`, and no `cfg(parallel)`
   variant even though den enables `parallel`. `Send`/`Sync` are asserted once on the *outer* future
   (`async.rs:305-306`, `:355-356`), never on a spawned task. A spawned future may therefore hold
   `Function<'js>`, `Class<'js, T>`, `Ctx<'js>` freely.
2. **hyper's h1 server path has no `Send` bound anywhere.** `hyper-1.11.0/src/server/conn/http1.rs:452-459`:
   `S: HttpService<IncomingBody>, S::Error: Into<Box<dyn StdError + Send + Sync>>, S::ResBody: 'static,
   <S::ResBody as Body>::Error: Into<...>, I: Read + Write + Unpin`. `S::Future` is unbounded and the token `Send`
   does not appear; `Service` (`service/service.rs`) and `HttpService` (`service/http.rs`) have none either.
   So a hyper connection can be a `ctx.spawn`ed future whose service captures a JS `Function<'js>` — and it is,
   probed: `[w1] /a -> JS:GET/A` / `[w1] /b -> JS:GET/B` / `[w1] /c -> JS:GET/C` / `[w1] idle() returned = true`,
   three keep-alive requests answered by `(m,p) => 'JS:' + m + p.toUpperCase()` with `rt.idle()` as the whole loop.
   **One exception, on the IO and not on the service:** `with_upgrades` is
   `pub fn with_upgrades(self) -> UpgradeableConnection<I, S> where I: Send` (`http1.rs:199-202`) and its `Future`
   impl asks `I: Read + Write + Unpin + Send + 'static` (`:541-547`). `TcpStream` and `TlsStream` satisfy both, so
   this is not fatal — but the sentence "the h1 path needs no `Send` at all" is false, and see fact 9 for what it
   costs.
3. **HTTP/2 also works, and this is now run-proven, not just compile-proven.** `hyper::rt::Executor<Fut>` is
   `fn execute(&self, fut: Fut)` with no bound (`rt/mod.rs:45-48`); the blanket `Http2ServerConnExec` impl asks only
   `E: Clone + Executor<H2Stream<F,B,E>> + Http2UpgradedExec<B::Data>` (`rt/bounds.rs:116-128`). `Send` enters only
   through `hyper_util::rt::TokioExecutor`'s own impl (`rt/tokio.rs`, `Fut: Send + 'static`), which we do not use.
   A four-line `LocalExec<'js>(Ctx<'js>)` forwarding to `Ctx::spawn` served real h2:
   `[w3] curl -> H2:/H2PATH [status 200 ver 2]` / `[w3] h2 conn ended err=None` / `[w3] idle() returned = true`.
   This closes the largest open risk the input research left behind.
4. **Exactly one hop is forced, and it is the response body.** `S::ResBody: 'static` (`http1.rs:456`;
   `http2.rs:310` `Bd: Body + 'static`) rejects a `Body` impl that borrows `'js`. Negative probe, verbatim:
   `error: lifetime may not live long enough` / ``returning this value requires that `'1` must outlive `'static` ``
   for `struct JsBody<'js> { _f: Function<'js> }`. The request body is *not* affected: `hyper::body::Incoming` is
   `'static` and is read on the realm thread with no channel at all.
5. **`mpsc::channel(1)` is necessary for response backpressure but is NOT sufficient, and the floor is megabytes.**
   With 7-byte chunks a stalled client sees `[w4] chunks produced while client stalled 300ms = 20 of 20` — hyper
   drains the body into its write buffer and the kernel socket buffer, neither of which the channel bounds. With
   256 KiB chunks backpressure does engage, at `[w4] chunks produced while client stalled 300ms = 13 of 20
   (256 KiB each)` then `[w4] drained rest (5243085 bytes); produced = 20 of 20` — i.e. about 3.4 MiB in flight
   before the producer parked. Any statement of the form "the channel bound is the backpressure" is wrong; the true
   statement is "the producer parks once channel + hyper buffer + socket buffer are full".
6. **Nothing polls a `ctx.spawn`ed future until `idle()` is running.** A first draft of the W2 probe awaited an
   in-flight signal from the service *before* calling `idle()` and deadlocked (`rc=124`): the accept loop had been
   spawned but never polled, so the connection was never accepted. Consequence for `serve()`: binding synchronously
   is fine and correct (`server.addr.port` is valid on return), but *accepting* only starts when control returns to
   the loop. Any Rust-side helper that binds and then awaits a request on the same task hangs.
7. **A `ctx.spawn`ed future keeps `idle()` pending; a `tokio::spawn`ed one does not.** `[claim1] tokio task alive,
   idle() returned = true` (an infinite `Handle::spawn` loop; `idle()` returned inside 500 ms) versus
   `[claim2] while spawned: idle() returned = false` / `[claim2] after release: idle() returned = true`. Mechanism:
   `async.rs:313` `idle()` returns `Poll::Ready(())` only on `SchedularPoll::Empty` (`:349`). So "a live server keeps
   the process alive" is ARCHITECTURE §7.5 rule 1 verbatim — *a queue the script opened stays open until the script
   closes it* — with no new liveness rule, no engine token, and no `ref()`/`unref()`.
8. **`idle()` holds the runtime mutex for its entire parked lifetime**, so a tokio task calling `async_with`/`with`
   blocks until `idle()` returns (09 §46-50 probe T12; `den-core/src/engine.rs:620-625` states the deadlock —
   *"A separate tokio task calling `async_with` during `idle()` would park on the mutex until idle returned"*;
   `src/app.rs:38-42` states the rule: *"The only way to do JS work during `idle()` is `ctx.spawn`"*). This
   eliminates the `LocalSet` option outright and is why the mailbox option pays a second `ctx.spawn` on top.
9. **Graceful drain is ~12 lines of hand-rolled machinery, because hyper-util's helper does not cover the h1
   connection den actually ships.** The hyper half is trivial: `Pin::new(&mut conn).graceful_shutdown()` is
   `self.conn.disable_keep_alive()` (`http1.rs:138-139`), and on `UpgradeableConnection` it forwards to the inner
   connection (`http1.rs:526-530`). The hyper-util half is not. `GracefulShutdown` is
   `{ tx: watch::Sender<()> }` (`graceful.rs:21-22`) whose `watch<C: GracefulConnection>` (`:42`, no `Send` bound)
   only accepts a `GracefulConnection`, and that trait is implemented for exactly four types:
   `http1::Connection` (`graceful.rs:166`), `http2::Connection` (`:182`), `auto::Connection` (`:199`) and
   `auto::UpgradeableConnection` (`:217`). **`http1::UpgradeableConnection` is not among them**, even though
   `private::Sealed` *is* implemented for it (`:250`). COMPILE-REFUTED against an earlier draft of §4.2 that had
   exactly that shape, verbatim: ``error[E0277]: the trait bound `UpgradeableConnection<TokioIo<TcpStream>, _>:
   GracefulConnection` is not satisfied ... required by a bound in `Watcher::watch` `` (`graceful.rs:92`). The
   trait is sealed, so the orphan rule forbids den implementing it; the only hyper-util type that supports
   upgrades *and* graceful is `auto::UpgradeableConnection`, which fact 10 rejects for `S::Future: 'static`.
   **So hyper-util's graceful helper and h1 upgrades are mutually exclusive, and den drops the helper for both
   arms.** The replacement is twelve lines: pin the connection, `select!` on the phase `watch`, call
   `Pin::new(&mut conn).graceful_shutdown()` (`http1.rs:526`), and hold one `mpsc::Sender` alive-token per
   connection so the accept loop learns when the last one ended by awaiting the receiver's close. Probed end to
   end with an async JS handler in flight, `UPG=1` mode: `[V1] drain completed` /
   `[V1] in-flight response body = "...JS:GET/HELLO"` / `rc=0`. The earlier `GracefulShutdown` run (no upgrades)
   drained the same case identically — `[w2] request in flight; sending close()` /
   `[w2] accept loop exited (stopped accepting)` / `[w2] conn drained after graceful_shutdown` /
   `[w2] in-flight response body = "JS:/HELLO"` / `[w2] idle() returned after close = true` — so the mechanism is
   equivalent; it is only the type plumbing that forces the hand roll.
10. **`hyper_util::server::conn::auto::Builder` is unusable here.** Both `serve_connection` (`auto/mod.rs:213-216`)
    and `serve_connection_with_upgrades` (`:256-267`) require `S::Future: 'static`, which forbids a service future
    borrowing `Ctx<'js>`; the latter also wants `I: ... + Send + 'static`. Use the raw `http1::Builder` /
    `http2::Builder`, which have no `S::Future` bound. Cleartext h1/h2c autodetect therefore means sniffing the
    24-byte preface ourselves (`auto/mod.rs:39` `const H2_PREFACE`, `:297` `fn read_version`); over TLS it is ALPN,
    which is better anyway (`read_version` blocks forever on a client that connects and sends nothing).
11. **BLOCKER, unrelated to hyper: a `ReadableStream` still reachable at runtime teardown aborts the process.**
    Probed on the working-tree binary, verbatim:
    `den: .../out/quickjs.c:2348: JS_FreeRuntime: Assertion 'list_empty(&rt->gc_obj_list)' failed.` `rc=134`.
    Reachability is what matters — `const s = new ReadableStream({}); globalThis.__s = s;` gives `rc=134`;
    `{ const s = new ReadableStream({}); }` gives `rc=0`; `const t = new TransformStream(); globalThis.__t = t;`
    gives `rc=134` while a bare unrooted `new TransformStream()` gives `rc=0`; `min-size-release` behaves
    identically (`rc=134`). Cause: `den-stdlib-whatwg/src/streams.rs:110` `let ctx = ctx.clone();` captured inside
    the controller's `error` closure, and `Ctx::clone` is `JS_DupContext`. A server holds one `ReadableStream` per
    in-flight request, so **every shutdown under traffic aborts** until this is fixed. Do not chase the symptom.
12. **den cannot set a cookie today.** Probed:
    `ctor getSetCookie: []` for `new Response("x", { headers: { "set-cookie": "a=1", ... } })`, then
    `after append: []` after `r.headers.append("set-cookie", "b=2")`, and `all: [["content-type","text/plain"]]`.
    Both paths are silent no-ops: `headers.rs:461`
    `fn is_forbidden_response_header(name: &str) -> bool { matches!(name, "set-cookie" | "set-cookie2") }`,
    applied at `:255` for `Guard::Response`, which the Response constructor installs (`lib.rs:434`), and the
    refusal returns `Ok(())` rather than throwing (`headers.rs:283-285`). Roadmap rule 4 forbids exactly this.
13. **Request headers must be handed in as a `Headers` *instance*, never a plain object.** `Guard::Request`
    (`headers.rs:244`) filters host/cookie/content-length/origin/referer/`sec-*`/`proxy-*`. Probed:
    `req kept: [["x-ok","1"]]` for an init object carrying host + cookie + x-ok, versus
    `req via Headers instance: [["cookie","k=v"],["host","example.com"]]` — `headers.rs:169-174` copies a `Headers`
    instance wholesale and bypasses `check_guard`. A server that builds requests from a plain object silently
    drops `Host` and `Cookie`.
14. **Three request shapes cannot be represented at all, so the server must answer them in Rust before
    construction.** Probed: `TRACE: TypeError: Forbidden method` (`request.rs:411` into `headers.rs:392`),
    `GET+body: TypeError: Body not allowed for GET or HEAD requests` (`request.rs:456`), and
    `101: RangeError: init['status'] must be in the range of 200 to 599, inclusive. 101` (`body.rs:302`). The
    first two are a 405/400 decided in Rust; the third is why a WebSocket upgrade cannot return a `Response`.
15. **The route grammar is `matchit`, and the reason is failure, not speed.** `matchit-0.8.4/src/error.rs:9-25`
    has `InsertError::{Conflict { with }, InvalidParamSegment, InvalidParam, InvalidCatchAll}`, so
    `/users/{id}` + `/users/{name}` is an error naming both keys *before the socket binds*. The alternative,
    URLPattern, cannot detect cross-pattern ambiguity at all — and den's own URLPattern binding cannot parse a
    full pattern string today: `URLPattern threw: TypeError: tokenizer error: invalid name; must be at least
    length 1 (at char 5)` for `new URLPattern("https://example.com/foo/:id")`. matchit has an empty
    `[dependencies]` section and 1729 lines of `src/`.
16. **One new crate for h1 + h2. One for routing. Six for TLS.** `cargo tree -e features -i hyper` shows
    `hyper v1.11.0` / `hyper feature "client"` / `reqwest v0.13.4` — hyper is already compiled into every den
    build, client-only. hyper-util 0.1.20, http 1.5.0, http-body-util 0.1.2, bytes 1.12.1, h2 0.4.18 are all in
    `Cargo.lock`. The one genuinely new crate is **httpdate**: `hyper-1.11.0/Cargo.toml` has
    `server = ["dep:httpdate", "dep:pin-project-lite", "dep:smallvec"]`, and `cargo tree -i httpdate --offline`
    in the den tree gives `error: package ID specification 'httpdate' did not match any packages` — it is in
    `Cargo.lock` but not in the current build graph. Every *other* dep the `server` feature set pulls
    (smallvec, pin-project-lite, atomic-waker, httparse, itoa, futures-channel, futures-core, tower-service) is
    already in the graph. httpdate is zero-dependency and trivial, but the number is one, not zero.
    hyper-util needs features `server` and `tokio` (the latter for `TokioIo` **and** `TokioTimer`, see fact 21);
    `server-graceful` is **not** enabled, because fact 9 drops the helper. matchit 0.8.4 is in the lock but *not*
    in the default build graph (`cargo tree -i matchit` gives
    `error: package ID specification 'matchit' did not match any packages`). rustls, ring and tokio-rustls likewise
    absent (`warning: nothing to print.`).
17. **TLS must be rustls, and this is a correctness argument.** `native_tls::TlsAcceptorBuilder::accept_alpn` is
    behind the non-default `alpn-accept` feature (`native-tls-0.2.18/Cargo.toml:55`, `src/lib.rs:562-564`) and has
    **zero** implementation in the Security.framework backend
    (`grep -c accept_alpn src/imp/security_framework.rs` gives `0`). den's current `TlsListener`
    (`den-stdlib-networking/src/tls.rs:31` `tls_acceptor`, one `Identity`) is therefore structurally incapable of
    ever serving h2 over TLS on macOS, and native-tls exposes no server-side SNI callback at all. Do **not** add
    `rustls-pemfile`: `rustls-pki-types 1.15.1` is already in the graph and ships
    `PemObject::{from_pem_slice, pem_slice_iter, from_pem_file}` (`src/pem.rs:21,29,37`).
18. **den already owns a server-side WebSocket engine that JS cannot reach.**
    `den-stdlib-networking/src/websocket.rs:360` `pub async fn accept_stream<S>(stream: S, supported: &[String])`
    negotiates subprotocols and drives the private I/O loop at `:509 async fn run<S>`. It is called from `:356`
    and from tests only. The hyper upgrade path needs one new constructor over
    `tungstenite::handshake::derive_accept_key` + `WebSocketStream::from_raw_socket(io, Role::Server, None)`, both
    already vendored with the `handshake` feature on. Zero new dependencies.
19. **Byte handoff into JS is `TypedArray::new_copy`, never `ArrayBuffer::new`.** A `Vec`-backed buffer reachable
    from script is double-freed by QuickJS. This is a mandatory memcpy per chunk on the hot path that Deno does not
    pay; measure before promising throughput parity.
20. **A synchronous JS handler stalls the entire realm** — every other connection, every timer, every `fetch` —
    because it runs while `idle()` holds the runtime mutex. Same shape as Deno's and Bun's single-threaded servers,
    except that on worker realms tokio runs `worker_threads(1)` (ARCHITECTURE §7.3), so there is not even a second
    thread to absorb it. `serve()` gives no isolation and the docs must say so. The same sentence applies to the
    per-connection **TLS handshake**, which §4.2 puts on the realm thread under the runtime mutex: a ~1 ms
    ECDSA/RSA handshake stalls every timer, every `fetch` and every other connection in the realm. It is still
    strictly better than `tls.rs:93-98`'s in-accept-loop handshake (which serialises *all* handshakes behind the
    slowest one), but "not head-of-line" is not "free" — and on a worker realm there is no second tokio thread at
    all. Measure it before promising connection rates (§7 q13).
21. **`header_read_timeout` without a timer PANICS, and without a timer hyper's own 30 s default is silently
    disabled.** RUN-REFUTED against an earlier draft of §4.2's `connection()` snippet, run verbatim
    (`/tmp/denplan/V1/probe/src/bin/timer_panic.rs`):
    ``thread 'main' panicked at hyper-1.11.0/src/common/time.rs:80:32: timeout `header_read_timeout` set, but no
    timer set``, `rc=101`. Source: `common/time.rs:76-79`,
    `Dur::Configured(Some(dur)) => match self { Time::Empty => panic!(...) }`. The fix is one builder call,
    `.timer(hyper_util::rt::TokioTimer::new())` (hyper-util feature `tokio`). The quieter half is worse: with no
    timer set, `common/time.rs:72-75` takes `Dur::Default(Some(dur)) => Time::Empty =>
    warn!("timeout `{}` has default, but no timer set"); None` — so a builder with no `.timer()` has **no header
    timeout at all**, neither configured nor default. A panic here lands inside a `ctx.spawn`ed future holding a
    live runtime, which is exactly the §4.3.9 hazard.

---

## 1. Why this is hard here, and why it stops being hard

### 1.1 The shape of the problem

An HTTP server is a machine that turns socket events into calls into a language runtime. In every other runtime
that is a scheduling question. Here it is a *type* question, because three facts collide:

- `Value`, `Object`, `Function`, `Promise` are `!Send` (09 §34, §205). A `Request` object cannot be moved to
  another thread, ever.
- `Ctx` is `Send` but `!Sync`, and the only way to obtain one from outside is `with`/`async_with`, which take the
  runtime mutex.
- `AsyncRuntime::idle()` — den's entire event loop (`src/app.rs:38-42`, ARCHITECTURE §2) — holds that mutex for its
  whole parked lifetime. A server process is parked in `idle()` essentially always.

So the naive design ("accept on tokio, call into JS when a request arrives") is not slow, it is a deadlock: the
tokio task calls `async_with`, parks on the mutex, and waits for `idle()` to return, which will not happen until
the server stops. `den-core/src/engine.rs:620-625` says this in the tree already.

The escape is one sentence from 09, and den's own comment repeats it: *"The only way to do JS work during `idle()`
is `ctx.spawn`"* (`src/app.rs:41-42`). Whatever the server is, it has to be a future the QuickJS scheduler owns.

### 1.2 Three seams, and why the first one wins

**(a) The whole server on the realm thread.** The accept loop is `ctx.spawn`ed; each connection is a `ctx.spawn`ed
future holding `hyper::server::conn::http1::Builder::serve_connection`; the service closure captures
`Function<'js>` and calls it inline. Nothing crosses a thread, so `!Send` never comes up.

The reason this is even legal is fact 2: hyper's h1 server bounds are `S: HttpService<IncomingBody>,
S::ResBody: 'static, I: Read + Write + Unpin` and nothing else (`http1.rs:452-459`). Most HTTP stacks would demand
`Send` on the service future for their executor's sake; hyper demands it only where an executor is actually
involved, which is h2, and even there the bound lives on `hyper_util::rt::TokioExecutor`, not on the trait
(`rt/mod.rs:45-48` versus `rt/tokio.rs`). Fact 3 shows a four-line local executor over `Ctx::spawn` satisfies h2 at
runtime.

**(b) A request mailbox.** hyper runs on tokio workers; a bounded `mpsc` carries plain-data envelopes
(`http::request::Parts` + a body channel + a reply `oneshot`) to a single `ctx.spawn`ed pump on the realm thread,
which spawns one dispatch future per request. This is not exotic — it is exactly what den already does twice:
`den-stdlib-worker/src/port.rs:354 ctx.spawn(Self::pump(...))`, documented at `:211` as *"the process-lifetime
mechanism for ports"*, and `den-stdlib-networking/src/websocket.rs` detaching the socket onto tokio with
`den-stdlib-whatwg/src/websocket.rs:127 ctx.spawn` as the matching ref pump.

It buys three real things: hyper-util's `Send`-only helpers (`auto::Builder`, `serve_connection_with_upgrades`,
`TokioExecutor`); TLS handshakes, header parsing and h2 framing off the realm on N cores; and socket-level liveness
while a synchronous JS handler is wedged. It costs a protocol layer, two channel hops and a cross-thread wake per
request — and, critically, it still needs a `ctx.spawn`ed pump (fact 7: a tokio task does not keep `idle()`
pending), so it pays seam (a)'s cost *plus* a `Send`-only hop.

**(c) A `LocalSet` on the realm thread.** Dead on arrival twice over: a LocalSet task can only reach a `Ctx` via
`async_with`, which parks on the mutex `idle()` holds (fact 8), and den is `#[tokio::main]` multi-thread
(`src/main.rs:57`) with no LocalSet, where `tokio::task::spawn_local` panics.

**Verdict: (a), and the margin is real but narrower than an earlier draft claimed.** Two of that draft's premises
were wrong. `GracefulShutdown::watch` has no `Send` bound (`graceful.rs:42`) but it does not accept
`http1::UpgradeableConnection` at all (fact 9), so seam (a) has to hand-roll the drain; and h1 `with_upgrades`
does need `Send` — on the IO, `I: Send` at `http1.rs:199-202` and `I: Read + Write + Unpin + Send + 'static` on the
`Future` impl at `:541-547` (fact 2). Neither changes the verdict. The `Send` bound is on `TcpStream`/`TlsStream`,
which satisfy it; the hand-rolled drain is twelve lines and den needs it for the h2 arm's deadline anyway, so it is
one mechanism rather than a helper plus a special case. What (b) still uniquely unlocks from hyper-util is
cleartext h2c autodetect, replaceable by a 24-byte preface sniff. What (b) genuinely buys is insurance against a
handler that blocks the realm — and that handler already freezes every timer, every `fetch` and every other
handler in *both* designs (fact 20). Buying a protocol layer to soften one symptom of an unfixable stall is
speculative. Seam (a) is also strictly better on the request body: `Incoming` is `'static` and lives on the realm
thread, so it needs no channel at all, whereas (b) must add one purely because it chose another thread.

Keep (b) on the shelf as the named fallback if h2 ever regresses (§7 q3).

### 1.3 Liveness and shutdown fall out; they are not designed

ARCHITECTURE §7.5 rule 1: *"A queue the script opened stays open until the script closes it."* `serve()` is such a
queue. The `ctx.spawn`ed accept loop is pending, so `idle()` does not resolve (fact 7), so den does not exit. That
is the whole liveness story — no `ref()`/`unref()`, which would in any case contradict the mechanism (the spawn
*is* the ref).

Shutdown follows 16 §4 and 17 §1.2 without adding anything: **drain is a property of resources, not of futures**,
and the resource owns its own close handle. `close()` flips a `watch`; the accept loop's `select!` breaks and
**drops the listener immediately**, so the kernel stops completing handshakes into the backlog and a fresh connect
is refused from that instant rather than at some point during the drain (§4.2); each connection future calls
`graceful_shutdown()` and awaits itself (drains in-flight); when the last connection future ends the scheduler is
empty and `idle()` returns. Probed in full at fact 9, including the case that matters — a request already inside an
async JS handler still completes (`in-flight response body = "JS:/HELLO"`).

This is the direct answer to 17 §7 q7, which recorded the gap in the negative: *"den has no HTTP server and
`TcpListener` has no `close()` ... an already-outstanding `accept()` ... pins `idle()` for ever ... a graceful
script cannot drain, and `exit()` in the listener is mandatory rather than polite."* The missing piece was a
resource method. **`close()` lands in phase 1, so 17 §4.2's published SIGINT recipe is wrong from phase 1 and is
rewritten there** — not in phase 6, where an earlier draft parked it. A recipe that still says `exit(0)` is
mandatory would have scripts throwing their drain away for five phases.

It answers only *half* of 17 §7 q7, though. `serve()` gets a close handle; a **raw `TcpListener` still does not**.
`den-stdlib-networking/src/socket.rs:59-69` gives `TcpListenerWrapper` exactly `local_addr`, `accept(self)` and the
static `listen` — no `close`, no `Symbol.dispose`. So the rewritten 17 §4.2 must keep the mandatory `exit()` for
scripts that hand-roll an accept loop, and say which recipe applies to which resource. Giving `TcpListener` its own
close handle is the same three-line `watch` shape and is not scheduled here (§7 q12).

---

## 2. Competitor surface

### 2.1 Deno.serve (2.9.4)

| Property | Behaviour | Evidence |
|---|---|---|
| Entry points | 7 overloads across tcp/unix/vsock/tls, handler-first and handler-in-options | `deno.d.ts:5892, 5940, 5990, 6054, 6103, 6141` |
| Bind | synchronous; port 0 resolved before return | `addr immediately after serve(): {"hostname":"127.0.0.1","port":34391,"transport":"tcp"}` |
| Options | `signal, onError, onListen, automaticCompression`, plus tcp `port(8000), hostname("0.0.0.0"), reusePort, tcpBacklog(511)` | `deno.d.ts:5748-5804` |
| **Timeouts** | **none at all** | after 6 s a silent socket and a truncated header block are both still open: `no header/idle timeout observed after 6s` |
| `request.signal` | **aborts on every completed request** by default; correct behaviour behind `--unstable-no-legacy-abort`; Deno prints its own runtime warning | a plain successful `curl` gives `ABORT / AbortError: The request has been cancelled.`; source `00_serve.ts:224-232`, gated `:488` |
| `info.completed` | documented to reject on disconnect; **resolved successfully on 4/4 disconnect probes** | `ABORT /slow AbortError...` then `completed OK /slow`; resolved at `00_serve.ts:628-690` *before* the bytes are written |
| Handler errors | throw / non-Response / rejection / undefined all give 500 `Internal Server Error` (cl 21), connection preserved | `/throw 500 "Internal Server Error" cl= 21`, `/nonresponse 500 ...` |
| Framing | string and bytes give content-length; stream gives chunked; 204 neither; HEAD headers only | `curl -D-` |
| Streaming | genuinely lazy both ways; a 200 MiB body to a 200 kB/s client produced **one** pull in 3 s | `bp pull 0 at ms 0` |
| `shutdown()` | listener closes at once, idle keep-alives FIN'd, in-flight finishes — **no drain deadline** | `shutdown() resolved after ms 1700`; `poll_fn(poll_complete).await` at `http_next.rs:5500-5520` |
| `signal` option | the **forceful** path; kills in-flight while the docs say "close the server" | `abort() @ 310` / `resp ERR fetch failed @ 318`; `http_next.rs:5450-5484` |
| h2 | zero-config: h2c preface sniff, h2 by ALPN; no JS-visible API | `curl --http2-prior-knowledge` gives `HTTP/2 200` |
| `req.url` | rebuilt from the **client-supplied Host header, unvalidated** | `Host: example.test:1234` gives `{"url":"http://example.test:1234/url"}` from a server bound to 127.0.0.1 |
| Lifetime | alive by default; `unref()`; `Symbol.asyncDispose`; idempotent shutdown; `AddrInUse` synchronous | `unref process exiting @ 5 (exit 0)`, `second bind: AddrInUse ... (os error 98)` |
| Implementation | JS callback invoked **synchronously** from inside the hyper service future, same thread, `Rc<HttpRecord>` via a `v8::External` | `service.rs:681-760`, `http_next.rs:4911`; accept loop is `deno_core::unsync::spawn` |

### 2.2 Bun.serve (1.3.9)

| Property | Behaviour | Evidence |
|---|---|---|
| Options | three XOR'd groups; `idleTimeout` default 10 s, `maxRequestBodySize` 128 MiB | `serve.d.ts:809-814, 678, 781` |
| Routes | object of path to `Response \| false \| handler \| per-method map`; **specificity, not declaration order** | `/*` declared first still lost to every more specific route |
| Wildcards | capture **nothing** | `/deep/a/b/c` gives `{"url":...,"params":{}}` |
| Method miss | route miss, falls through, bare 404, **no `Allow`** | `PUT /methods (no fetch fallback) 404 "" \| content-length: 0` |
| Auto-HEAD | none; HEAD on a GET-only route 404s | `HEAD /methods 404 ""`, `getCalls after HEAD = 0` |
| Param decoding | percent-decoded **including `%2F` and `%00`** | `/files/%2E%2E%2F%2E%2E%2Fetc%2Fpasswd` gives `{"name":"../../etc/passwd"}` |
| Pattern validation | `/a/*/b` silently dead; `:` registers a param named `""`; `:x(\d+)` registers a param literally named `x(\d+)` | `wildcard mid-path OK` (nothing matched); `regex-ish OK /a/1=200:{"x(\\d+)":"1"}` |
| Non-Response return | **200 "Welcome to Bun!"** page | `/bad-return -> 200 ... "Welcome to Bun! To get started, return a Response object."` |
| `stop(false)` | listener closes, but **already-open keep-alive sockets still get fresh requests served** | `506ms new request after stop REJECTED` yet `506ms keep-alive reuse after stop -> "HTTP/1.1 200 OK"` |
| `stop(true)` | socket killed instantly, handler **keeps running**, promise still waits for it | `308ms in-flight REJECTED` / `3014ms slow handler finished` / `3014ms stop() promise RESOLVED` |
| `reload()` | mutates in place; omitting `routes` silently **drops static routes** while keeping function routes | `reload({},C) 200:fetch-C 200:fetch-C` |
| Disposal | `Symbol.dispose` only — `await using` cannot await the drain | `Symbol.asyncDispose? undefined` |
| WS `send()` | one integer for three meanings: `n` written, `-1` buffered, `0` dropped | histogram `[[1,39],[-1,5],[0,2956]]`, `peakBuffered 262564` at a 256 KiB limit |
| `closeOnBackpressureLimit` | deferred to the next JS turn: 2956 dropped sends with `readyState === 1` first | same run, `"finalReadyState":1`; close 1006 only after the handler returned |
| TLS | `tls` accepts an **array**, i.e. a real SNI table; the first entry is the fallback | `SNI=unknown.test -> peer CN=alpha.test` |
| `server.fetch()` | exists; Bun's own types call it inconsistent | `serve.d.ts:877-880` |

### 2.3 node:http

Legacy `(req, res)` with `res.writeHead`/`res.end`; two stream types instead of one; `server.close()` waits on
keep-alive sockets indefinitely (`closeAllConnections`/`closeIdleConnections` were added late); h2 is a separate
module with a raw session/stream API. The **one thing Node gets right that neither of the others does** is the
three-timeout model — `headersTimeout`, `requestTimeout`, `keepAliveTimeout` — which is what actually stops
slowloris. 15 §6 already rules the `(req, res)` model out of scope for den.

### 2.4 What the three teach

Take: routes as data (Bun), synchronous bind with the address on the handle (Deno/Bun), a disconnect signal
(both), `shutdown()`/`finished` (Deno), transparent h2 (Deno), the three timeouts (Node).
Refuse: Deno's abort-on-success default and its deadline-free drain; Bun's keep-alive leak on stop, its
`stop(true)`, its in-place `reload()`, its uncapturable wildcards, its `%2F` decoding, its silent pattern
acceptance, its marketing-page fallback and its magic-number `send()`; Node's whole surface.

---

## 3. What den has, and what it must gain

### 3.1 Reusable as-is

| Piece | Where | Note |
|---|---|---|
| `Request` / `Response` / `Headers` classes | `den-stdlib-whatwg-fetch/src/{request,lib,headers}.rs` | `Request::new` is `pub` (`request.rs:246`); `Headers::new` is `pub` with `Guard::None` |
| `ReadableStream` with a native async `pull` | `den-stdlib-whatwg/src/streams.rs:378`, `:139-171` | `pull_if_empty` is re-entrancy-guarded (`:152 state.pulling = true`) and `Host::maybe_await`s a Promise result (`:165`), so demand-driven request bodies are free. `den-stdlib-whatwg-fetch/src/body.rs:234-266` already does exactly this for fetch |
| `AbortController` | `den-stdlib-worker/src/abort.rs:208 new`, `:214 abort` | both `pub`; a `ctx.spawn`ed future can trip a request's signal |
| `SocketAddr` class | `den-stdlib-networking/src/socket_addr.rs` | do not invent an address bag |
| server WebSocket engine | `den-stdlib-networking/src/websocket.rs:360, :509` | works, negotiates subprotocols, unreachable from JS (15 §3.6 calls it exactly that) |
| the `ctx.spawn` pump idiom | `den-stdlib-worker/src/port.rs:211, :354` | the precedent this design follows |

### 3.2 Broken, and in the way

| Defect | Evidence | Blocks |
|---|---|---|
| a reachable `ReadableStream`/`TransformStream` aborts at teardown | `JS_FreeRuntime: Assertion 'list_empty(&rt->gc_obj_list)' failed`, `rc=134`; cause `streams.rs:110` captured cloned `Ctx` | **everything** — one stream per in-flight request |
| Set-Cookie silently dropped on both paths | `ctor getSetCookie: []`, `after append: []`; `headers.rs:461`, `:255`, `lib.rs:434`, silent at `:283-285` | any server that sets a cookie |
| header guard eats `Host`/`Cookie` from a plain init object | `req kept: [["x-ok","1"]]` versus `req via Headers instance: [["cookie","k=v"],["host","example.com"]]` | request construction |
| `pipeThrough` never pumps; `pipeTo` buffers everything and never closes the sink; `tee`/`from`/async-iteration absent in JS | `streams.rs:560-579` (lock, return, no pump), `:581-634` (`read_all_bytes` then one `write`); probed `tee? undefined asyncIterator? undefined` | middleware, compression, SSE — none of which this document ships |
| URLPattern cannot parse a full pattern string | `TypeError: tokenizer error: invalid name; must be at least length 1 (at char 5)` | using URLPattern as the route grammar |
| `fetch` buffers request bodies into a `Vec` before sending; pre-buffers responses under an 8 MB heuristic | `fetch_op.rs:1062`, `:1636` | client and server sharing one stream core (15 §4.3) |
| `bad-chunk` test hooks compiled into shipping fetch code | `lib.rs:261, :270`, `body.rs:586` | nothing — but a real server lets them be deleted |
| `TlsListener::accept` handshakes **inline in the accept loop** | `tls.rs:93-98` | head-of-line DoS; not to be reproduced |
| hand-rolled loopback HTTP/1.1 server, 211 lines | `den-stdlib-whatwg/src/local_http.rs:47` | exists only because den had no server; deleted in phase 6 |

### 3.3 Must gain

`serve()` itself; a bound address on a handle; `close({drainMs})`/`finished`/`asyncDispose`; a truthful disconnect
signal; the three timeouts; body limits; a route table; TLS with ALPN/SNI/mTLS; a reachable WebSocket upgrade; and
one typed error table shared with the fetch client.

---

## 4. The design

### 4.1 JS surface (`types/den-http.d.ts`)

Import-only. `den:http` goes in the resolver and loader lists (`engine.rs:259-319`, `:342-395`) and **not** in the
`evaluate_stdlib_module!` list (`:471-505`), exactly like `den:fs`, `den:networking` and `den:sqlite` — so it
installs no globals, an untrusted module must import it explicitly, and roadmap rule 1 is satisfied by construction
(nothing is evaluated).

```ts
declare module "den:http" {
  import type { SocketAddr } from "den:networking";

  // ---- handler ----
  type Handler<P = Readonly<Record<string, string>>> =
    (request: Request, connection: ConnectionInfo<P>) => Response | Promise<Response>;

  interface ConnectionInfo<P = Readonly<Record<string, string>>> {
    /** matchit params, RAW (never percent-decoded); {} for the fallback handler. */
    readonly params: P;
    readonly remote: SocketAddr;
    readonly local: SocketAddr;
    readonly alpn: "http/1.1" | "h2" | null;
    readonly sni: string | null;
    readonly peerCertificates: readonly Uint8Array<ArrayBuffer>[] | null;   // mTLS
    /**
     * Resolves when the response body's terminal frame was handed to the
     * transport; rejects HttpError{kind:"Aborted"} if it was not.
     * NOT an ACK from the peer - den promises the weaker true thing.
     * Unconditional: DenBody is hand-written, so a fixed body is polled
     * through the same poll_frame as a stream and the Liveness guard costs
     * one oneshot per request either way (§7 q1).
     */
    readonly completed: Promise<void>;
  }

  // ---- routes as data. matchit grammar: {id}, {*rest}. A ':' in a key throws. ----
  type Method = "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS";
  type Route<P> =
    | Response                                          // pre-serialised, never enters JS
    | Handler<P>                                        // any method
    | Partial<Record<Method, Response | Handler<P>>>;   // method miss => 405 + Allow

  /** Per-route param inference from the literal key. */
  type Params<K extends string> =
    K extends `${string}{*${infer T}}${string}` ? Record<T, string>
    : K extends `${infer H}{${infer N}}${infer T}` ? Record<N, string> & Params<`${H}${T}`>
    : {};

  type Listen =
    | { readonly host?: string; readonly port?: number;
        readonly backlog?: number; readonly reusePort?: boolean }
    | { readonly unix: string; readonly unlinkOnBind?: boolean; readonly mode?: number };

  interface CertKey {
    readonly cert: string | Uint8Array<ArrayBuffer>;    // PEM chain
    readonly key: string | Uint8Array<ArrayBuffer>;     // PEM PKCS#8/PKCS#1
  }
  interface TlsConfig extends CertKey {
    /** SNI table; an absent hostname falls back to the top-level cert/key. */
    readonly sni?: Readonly<Record<string, CertKey>>;
    readonly alpn?: readonly ("h2" | "http/1.1")[];     // default ["h2", "http/1.1"]
    /** mTLS: PEM roots. Presence makes a client certificate REQUIRED. */
    readonly clientCa?: string | Uint8Array<ArrayBuffer>;
  }

  interface Limits {
    readonly headerBytes?: number;            // default 64 KiB
    readonly bodyBytes?: number;              // default 128 MiB -> typed 413
    readonly headersTimeout?: number;         // ms, default 10_000   (slowloris)
    readonly requestTimeout?: number;         // ms, default 0 = off
    readonly idleTimeout?: number;            // ms, default 60_000
    readonly connections?: number;            // default 4096
    readonly concurrentRequests?: number;     // default 1024; bounds the spawner
  }

  interface ServeOptions<R = {}> {
    readonly listen?: Listen;                 // default { host: "127.0.0.1", port: 8000 }
    readonly routes?: { readonly [K in keyof R]: Route<Params<K & string>> };
    readonly fetch?: Handler;                 // fallback; required when `routes` is absent
    readonly tls?: TlsConfig;
    readonly http?: "auto" | "h1" | "h2c";    // cleartext only; TLS picks by ALPN
    readonly limits?: Limits;
    readonly onError?: (error: unknown, connection: ConnectionInfo) => Response | Promise<Response>;
    // reserved, unimplemented until den:permissions exists - see §7 q5:
    // readonly bind?: NetBind;
  }

  interface Server extends AsyncDisposable {
    readonly addr: SocketAddr | { readonly unix: string };  // port 0 already resolved
    readonly url: string;
    readonly finished: Promise<void>;                       // resolves when drained
    readonly pending: { readonly requests: number; readonly connections: number };

    /** Stop accepting (the listener is dropped at once), FIN idle keep-alives,
     *  drain in-flight; at `drainMs` abort the survivors' request signals and
     *  drop their sockets. `drainMs: 0` is the forceful path. Idempotent. */
    close(options?: { readonly drainMs?: number }): Promise<void>;
    [Symbol.asyncDispose](): Promise<void>;                 // === close()
  }

  /** Binds synchronously; throws HttpError{kind:"Bind"|"AddrInUse"|"Route"} before returning. */
  export function serve<const R>(options: ServeOptions<R>): Server;

  /** 101 is outside den's 200..=599 Response clamp, so the upgrade is its own verb. */
  export function upgradeWebSocket(
    request: Request,
    options?: { readonly protocols?: readonly string[]; readonly headers?: HeadersInit },
  ): { readonly response: Response; readonly socket: ServerWebSocket };

  interface ServerWebSocket
    extends AsyncIterable<string | Uint8Array<ArrayBuffer>>, AsyncDisposable {
    send(data: string | Uint8Array<ArrayBuffer>): SendResult;
    close(code?: number, reason?: string): void;
    readonly bufferedAmount: number;
    readonly readyState: 0 | 1 | 2 | 3;
    readonly protocol: string | null;
  }
  /** Bun crams three meanings into one integer; den does not. */
  type SendResult =
    | { readonly status: "sent"; readonly bytes: number }
    | { readonly status: "buffered"; readonly bytes: number; readonly buffered: number }
    | { readonly status: "dropped"; readonly reason: "backpressure" | "closed" };

  export class HttpError extends Error {
    readonly kind:
      | "Bind" | "AddrInUse" | "Tls" | "Route"
      | "HeadersTimeout" | "RequestTimeout" | "IdleTimeout"
      | "BodyTooLarge" | "TooManyConnections"
      | "Aborted" | "Protocol" | "Handler";
    readonly cause?: unknown;
  }
}
```

Usage:

```ts
import { serve } from "den:http";
await using server = serve({
  routes: {
    "/health": new Response("ok"),                                     // never enters JS
    "/users/{id}": { GET: (_r, { params }) => Response.json(params) }, // params.id: string
    "/static/{*path}": (_r, { params }) => serveFile(params.path),
  },
  fetch: () => new Response("not found", { status: 404 }),
  listen: { port: 0 },
});
console.log(server.url);
await server.close({ drainMs: 5_000 });
```

**`request.url` is built from the listener, not from the client.** §2.1 records Deno's defect: it rebuilds the
absolute URL from the client-supplied `Host` header with no validation, so a server bound to `127.0.0.1` answering
`Host: example.test:1234` hands the handler `{"url":"http://example.test:1234/url"}`. Every downstream absolute-URL
comparison — an origin check, a redirect target, a cache key, a signed-URL verification — is then attacker-chosen.
15 §3.6 has this as a P1 row ("Absolute request URL on the server (`req.url`) and Host/`:authority`/Forwarded
reconstruction"), and `bridge.rs` cannot be written without deciding it, so it is decided here:

- **Scheme** is `https` if the connection is TLS, else `http`. Not from `X-Forwarded-Proto`.
- **Authority** is the listener's own bound `SocketAddr` (the same value on `server.addr`), or the configured
  `listen.host` when one was given. Never the `Host` header, never `:authority`, never `Forwarded` or any
  `X-Forwarded-*`. On a unix socket the authority is the literal `localhost`.
- **Path and query** are the request target verbatim, and only origin-form is accepted; an absolute-form target
  (`GET http://elsewhere/ HTTP/1.1`, legal for proxies) is a 400 `HttpError{kind:"Protocol"}` in Rust before
  construction, because den is not a proxy.
- The client's own values are **not hidden** — `request.headers.get("host")` and, on h2, the synthesised `host`
  from `:authority` are both present (fact 13 is why the headers must be handed in as a `Headers` instance, or
  the guard eats exactly these). A vhosting router reads that header explicitly and takes responsibility for it.

Consequence to document: den behind a TLS-terminating reverse proxy reports `http://127.0.0.1:8000/...`, not the
public URL. That is the correct default — the public URL is proxy configuration, not request data — and the fix is
a future `trustedProxy` option, which is not designed here (§7 q14).

### 4.2 Rust architecture

New crate `den-stdlib-http`, **two** cargo features on den-core: `stdlib-http` (added to the `stdlib` aggregate,
which `den-core/Cargo.toml:41` puts in `default`) and `stdlib-http-tls` (**not** in the aggregate, phase 5).

The split is not deferrable. `stdlib` is in `default`, so folding TLS into `stdlib-http` makes rustls,
rustls-webpki, ring, untrusted, subtle and tokio-rustls — all six currently absent from the graph, verified by
`cargo tree -i` — hard dependencies of a plain `cargo build` and of every downstream embedder, at the measured
+1.58 MiB stripped (§7 q6). 15 §4.7 states the opposite policy in as many words: *"Heavy backends (rustls, hickory,
ICU4X, brotli/zstd, argon2) join as feature packs."* Splitting is three lines of `Cargo.toml`; un-shipping a
default dependency later is not. `serve({ tls })` without the feature is
`HttpError{kind:"Tls"}` naming the missing feature — roadmap rule 4, fail explicitly.

```
den-stdlib-http/src/
  lib.rs        #[rquickjs::module] js_http - serve, upgradeWebSocket, class Server, class HttpError
  options.rs    impl FromJs for ServeOptions/Listen/TlsConfig/Limits  (den-stdlib-fs/src/lib.rs:124-131 idiom)
  serve.rs      bind + the Server class + the watch phase machine
  conn.rs       one connection = one ctx.spawn'ed future; LocalExec for h2
  dispatch.rs   route lookup -> JS call -> Response.  MUST NOT PANIC.
  bridge.rs     http::Request<Incoming> -> den Request; den Response -> http::Response
  body.rs       DenBody (the 'static response body), Incoming -> ReadableStream pull, Liveness
  router.rs     matchit table + method map + pre-rendered static routes
  tls.rs        rustls ServerConfig: ALPN, SNI resolver, mTLS
  upgrade.rs    derive_accept_key + hyper::upgrade::on -> NativeWebSocket::from_upgraded
  error.rs      HttpError kinds
```

The handle. `matchit::Router` exposes no value iterator — `grep -n 'pub fn ' matchit-0.8.4/src/router.rs` gives
exactly `new` (:22), `insert` (:39), `at` (:58), `at_mut` (:85), `remove` (:128) and `check_priorities` (:133);
there is no `merge` in 0.8.4 — so the trie stores `usize` and a side `Vec` holds the values, axum's `RouteId` shape.

```rust
// Three states, all three read. `Closing` is gone: it never had a reader.
#[derive(Clone, Copy, PartialEq)]
enum Phase { Serving, Draining { deadline: Option<Instant> }, Done }

#[rquickjs::class(rename = "HttpServer")]
pub struct Server {
  addr:     den_stdlib_networking::socket_addr::SocketAddrWrapper,
  url:      String,
  // ONE latch, not two. `close()` writes Draining, the accept loop writes Done,
  // and `finished` is `phase.subscribe().wait_for(|p| *p == Phase::Done)` --
  // idempotent and multi-await for free, so no CancellationToken alongside it.
  phase:    tokio::sync::watch::Sender<Phase>,
  counters: std::rc::Rc<Counters>,
}

/// Not a JS class: an `Rc` held by the accept loop and by every spawned
/// dispatch future, which is what roots the `Function<'js>` values by
/// refcount. Nothing traces it, so it carries no `#[qjs(...)]` annotations.
pub struct Dispatch<'js> {
  paths:    matchit::Router<usize>,
  routes:   Vec<RouteEntry<'js>>,
  fallback: Option<rquickjs::Function<'js>>,
  on_error: Option<rquickjs::Function<'js>>,
  limits:   Limits,
}
struct RouteEntry<'js> {
  methods: indexmap::IndexMap<String, rquickjs::Function<'js>>, // "" = ANY; keys == the Allow header
  literal: Option<PreRendered>,                                 // status + header bytes + body, built at serve()
}
```

The accept loop is a free `fn` with a **named** `'js`, not an inline closure: `Ctx<'js>` is invariant and an
inferred closure lifetime fails with *"makes the generic argument `'_` invariant"*.

```rust
fn accept_loop<'js>(
  ctx: Ctx<'js>, listener: tokio::net::TcpListener,
  tls: Option<std::sync::Arc<rustls::ServerConfig>>,
  dispatch: Rc<Dispatch<'js>>, counters: Rc<Counters>,
  mut phase_rx: watch::Receiver<Phase>, phase_tx: watch::Sender<Phase>,
) -> impl Future<Output = ()> + 'js {
  async move {
    // The alive-token replaces hyper-util's GracefulShutdown (fact 9). Every
    // connection future owns a clone of `alive_tx`; the receiver yields None
    // exactly when the last one has been dropped.
    let (alive_tx, mut alive_rx) = tokio::sync::mpsc::channel::<Never>(1);
    let listener = {
      let listener = listener;                       // moved in, dropped below
      loop {
        let (io, peer) = tokio::select! {
          _ = phase_rx.changed()  => break,
          r = listener.accept()   => match r { Ok(v) => v, Err(_) => break },
        };
        if counters.connections() >= dispatch.limits.connections { drop(io); continue; }
        ctx.clone().spawn(connection(ctx.clone(), io, peer, /* ... */ alive_tx.clone()));
      }
      listener
    };
    // STOP ACCEPTING MEANS STOP ACCEPTING. Dropping the listener here closes the
    // socket before the drain begins; leaving it alive for `drainMs` would have the
    // kernel keep completing handshakes into a backlog nobody ever accepts from,
    // and would make "a second connect is refused" a race rather than a fact.
    drop(listener);
    drop(alive_tx);                                   // our own clone, or recv never ends
    // the deadline is the ONLY thing Deno's shutdown() cannot express
    tokio::select! {
      _ = alive_rx.recv()             => {}           // None: every connection ended
      _ = drain_deadline(&mut phase_rx) => { /* abort survivors' signals, drop sockets */ }
    }
    let _ = phase_tx.send(Phase::Done);                // this is what `finished` awaits
  }
}
```

Ordering matters and is the point: **listener dropped, then drain, then `Done`.** An earlier draft kept the
listener alive as a local until the whole future returned — i.e. past the drain and past the resolution of
`close()`/`finished` — so during `drainMs` the socket was still bound and accepting, and a client connecting in the
window between `close()` resolving and the future returning got a completed handshake instead of `ECONNREFUSED`.
The probe only ever saw `Connection refused` because it tested after `idle()` had returned.

One connection. Its drain is the twelve lines fact 9 forces, and h1 and h2 use the *same* twelve lines:

```rust
async fn connection<'js>(
  ctx: Ctx<'js>, io: TcpStream, peer: SocketAddr,
  mut phase: watch::Receiver<Phase>, _alive: mpsc::Sender<Never>, /* ... */
) {
  // The TLS handshake happens HERE, per connection - never in the accept loop.
  // den-stdlib-networking/src/tls.rs:93-98 handshakes inline; that is a head-of-line
  // DoS and is deliberately not reproduced. It is still ON the realm thread under the
  // runtime mutex, and it still stalls every timer for its duration (fact 20).
  let io = match tls { Some(cfg) => Either::Tls(cfg.accept(io).await?), None => Either::Plain(io) };
  let svc = hyper::service::service_fn(move |req| dispatch_one(ctx.clone(), state.clone(), req, peer));
  match alpn {
    Some(b"h2") => drive(http2::Builder::new(LocalExec(ctx.clone()))
                          .timer(TokioTimer::new())
                          .serve_connection(TokioIo::new(io), svc), phase).await,
    _           => drive(http1::Builder::new()
                          // MANDATORY. Without a timer `header_read_timeout` PANICS
                          // (hyper common/time.rs:80) and even hyper's built-in 30 s
                          // default is silently discarded (time.rs:72-75). Fact 21.
                          .timer(hyper_util::rt::TokioTimer::new())
                          .header_read_timeout(limits.headers_timeout)
                          .serve_connection(TokioIo::new(io), svc)
                          .with_upgrades(), phase).await,
  }
}

/// The whole graceful-drain mechanism, and the replacement for hyper-util's
/// GracefulShutdown, which structurally cannot accept an
/// http1::UpgradeableConnection (fact 9). `graceful_shutdown` is an INHERENT
/// method on both connection types, not a shared trait, so `drive` is either
/// generic over a three-line private trait or written twice; the body is this.
async fn drive<C>(conn: C, mut phase: watch::Receiver<Phase>) -> hyper::Result<()> {
  let mut conn = std::pin::pin!(conn);
  loop {
    tokio::select! {
      result = conn.as_mut()   => return result,
      _      = phase.changed() => conn.as_mut().graceful_shutdown(),
      //  http1.rs:526 -> disable_keep_alive (http1.rs:138-139). The connection
      //  then finishes its in-flight exchange and resolves on its own. A second
      //  phase change (Draining -> Done) can re-enter this arm; harmless,
      //  disable_keep_alive is idempotent.
    }
  }
}

#[derive(Clone)]
struct LocalExec<'js>(Ctx<'js>);
impl<'js, F: Future<Output = ()> + 'js> hyper::rt::Executor<F> for LocalExec<'js> {
  fn execute(&self, fut: F) { self.0.clone().spawn(fut) }
}
```

`LocalExec` is fact 3's four lines, run-proven (`[w3] curl -> H2:/H2PATH [status 200 ver 2]`).

### 4.3 Threading model, end to end

1. `serve()` is a **synchronous** Rust function. It binds a `std::net::TcpListener` (port 0 already resolved, so
   `server.addr.port` is valid on return — Deno's behaviour, and it lets den drop `onListen` entirely), sets
   nonblocking, converts to `tokio::net::TcpListener`, compiles the matchit table (any `InsertError` becomes
   `HttpError{kind:"Route"}` thrown *before the socket exists*), then `ctx.clone().spawn(accept_loop(...))` and
   returns the handle. **Accepting begins when control returns to `idle()`** (fact 6).
2. That spawned future is what keeps den alive (fact 7 = ARCHITECTURE §7.5 rule 1). No new rule, no `ref`/`unref`.
3. A socket event reaches JS with **zero hops**: `listener.accept().await` is polled by `idle()`'s poll of the
   scheduler, with the runtime mutex held; the connection future holds `serve_connection`, whose service captures
   `Ctx<'js>` and `Rc<Dispatch<'js>>` directly, so the den `Request` is built and `handler.call()` happens inline.
   `!Send` is never exercised.
4. Awaiting the handler's Promise works because `idle()` drains `execute_pending_job()` to exhaustion before each
   scheduler poll (`async.rs:320`, `:349`), so microtasks and spawned futures interleave. Proven with a handler
   whose Promise is settled by a `ctx.spawn`ed 200 ms timer rather than a microtask:
   `[w2] in-flight response body = "JS:/HELLO"`.
5. **Bodies, and their asymmetry.**
   *Wire to JS:* no channel. `Incoming` is `'static` and lives here; the request's `ReadableStream` gets a native
   async `pull` that polls one frame and enqueues `TypedArray::new_copy` (fact 19). `streams.rs:139-171` already
   guards re-entrancy and awaits a Promise-returning pull, so a handler that never reads the body never buffers it,
   and `limits.bodyBytes` is enforced in Rust before bytes reach JS.
   *JS to wire:* forced `'static` (fact 4). A non-streaming Response is `DenBody::Full(Bytes)` and allocates **no
   channel at all** — the common request costs one path, not a degenerate one-chunk stream. A streaming Response is
   `DenBody::Stream(mpsc::Receiver<Result<Frame<Bytes>, HttpError>>)` at capacity 1, fed by a second `ctx.spawn`ed
   pump calling `reader.read()`. Per fact 5, state the bound honestly: the producer parks once channel + hyper
   buffer + socket buffer are full (about 3.4 MiB measured), not after one chunk.
   **`DenBody` must implement `Body::size_hint`, or every response goes out chunked.** Framing does not fall out
   of the variant; it falls out of that one method. Measured on the V1 probe with the same `DenBody::Full(Bytes)`:
   with the default `size_hint`, hyper writes
   `HTTP/1.1 200 OK|connection: close|transfer-encoding: chunked|...`; with `SizeHint::with_exact(len)` it writes
   `HTTP/1.1 200 OK|connection: close|content-length: 12|...`. So `Full` returns
   `SizeHint::with_exact(bytes.len() as u64)` and `Stream` returns the default. The §4.3.5 claim that "the common
   request costs one path" depends on it.
6. **Disconnect and `completed` are one primitive.** The response body owns an armed drop-guard:

   ```rust
   /// Dropping this armed means "the peer never got the whole body".
   struct Liveness(Option<tokio::sync::oneshot::Sender<()>>);
   impl Liveness { fn disarm(&mut self) { self.0.take(); } }   // -> receiver sees Err(RecvError)
   impl Drop for Liveness {
     fn drop(&mut self) { if let Some(tx) = self.0.take() { let _ = tx.send(()); } }
   }
   ```

   `DenBody::poll_frame` disarms on the terminal `None`. The per-request dispatch future `select!`s the receiver:
   `Ok(())` means `AbortController::abort` (`abort.rs:214`) and reject `completed` with `HttpError{kind:"Aborted"}`;
   `Err(_)` means resolve `completed`. One oneshot, both features, and unlike Deno (`00_serve.ts:628-690`) it is
   derived from the body's terminal state rather than settled optimistically at handler return.
7. **Admission control is not optional — but not for the reason an earlier draft gave.** rquickjs's `Schedular` is
   **waker-driven, not a per-poll walk**: `schedular.rs:130-160` returns `Empty` when the all-list is empty,
   re-registers the waker (`:145`), and then pops **only** from the waker-fed `should_poll` queue (`:152`,
   `Pin::new_unchecked(&*self.should_poll).pop()`); a task nobody woke is never touched, and a task that returns
   `Ready` is unlinked by `pop_task_all` (`:106-128`, `self.len.set(self.len.get() - 1)`). Per-poll cost is
   O(woken tasks), not O(in-flight requests), so an implementer told otherwise will optimise the wrong thing.
   The cap survives on the two grounds that are actually true: **memory** — one `ctx.spawn`ed dispatch future per
   in-flight request, each holding a `Request`, a `ReadableStream`, a `Liveness` and whatever the handler closed
   over, with no ceiling but the accept rate — and **fairness**, since every one of those futures competes for the
   single realm thread, so an unbounded in-flight count converts a latency problem into a stall. `limits.concurrentRequests`
   (a semaphore at accept) and `limits.connections` are load-bearing from day one.
8. **What this does not give: isolation.** Fact 20. Say it in the docs.
9. **Panics.** A panic in a `ctx.spawn`ed connection future unwinds through the QuickJS scheduler on a thread
   holding a live runtime — the hazard ARCHITECTURE §7.3 catches for worker threads. Every JS exception becomes
   `onError` or a 500 via `ctx.catch()`; every `unwrap()` in `bridge.rs` is a process kill. This is the largest
   correctness risk in the implementation and it is concentrated in one file.

### 4.4 TLS

All of this is behind `stdlib-http-tls`, which is **not** in the `stdlib` aggregate and therefore not in
`default` (§4.2, §7 q6); `serve({ tls })` without the feature throws `HttpError{kind:"Tls"}` naming it.

`rustls` with `default-features = false, features = ["ring", "std", "tls12", "logging"]` plus `tokio-rustls`
likewise. Six new crates (rustls, rustls-webpki, ring, untrusted, subtle, tokio-rustls) at versions the lock already
resolved (rustls 0.23.43, ring 0.17.14, tokio-rustls 0.26.1); `rustls-pki-types` 1.15.1, `zeroize` and `once_cell`
are already present. ring rather than the aws-lc-rs default: no C/CMake toolchain and roughly 4x faster to build
(measured 7.22 s / 22 crates versus 27.54 s / 27 crates for a rustls-only scratch crate). PEM through `PemObject`
(fact 17), **not** rustls-pemfile.

```
ServerConfig::builder_with_provider(ring)
  .with_protocol_versions(DEFAULT_VERSIONS)?
  .with_client_cert_verifier(WebPkiClientVerifier::builder(roots).build()?)   // or .with_no_client_auth()
  .with_single_cert(certs, key)?                                             // or .with_cert_resolver(sni_map)
cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
```

Two rules. **The protocol is pinned from ALPN, not sniffed** — `tls.get_ref().1.alpn_protocol()` selects
`http2::Builder` or `http1::Builder` directly, which sidesteps both `auto::Builder`'s `S::Future: 'static` bound
(fact 10) and its connect-and-send-nothing hang. **SNI is a data map**, compiled at `serve()` into a `Send + Sync`
resolver: `rustls::server::ResolvesServerCert` is `Debug + Send + Sync`, so a JS callback can never be one. If a
dynamic hook is ever wanted the escape hatch is `tokio_rustls::LazyConfigAcceptor`, which stops after the
ClientHello and lets the config be chosen asynchronously on the realm thread; not built now.

Call `rustls::crypto::ring::default_provider().install_default()` once at module init. It is one line of insurance:
if anything ever enables reqwest's `rustls-tls` (which pulls rustls's aws-lc-rs default), the provider features
unify and rustls panics at the first `ServerConfig::builder()` — silent at compile time, fatal at runtime.

den keeps native-tls for **outbound** TLS (reqwest, WebSocket client). Two TLS stacks is the correct temporary
state; a client migration touches nine files and needs a new trust-store crate (§7 q7).

### 4.5 Routing: matchit, and the honest cost

**Verdict: matchit `{id}` / `{*rest}`, not URLPattern `:id`.** The primary reason is fact 15 — conflicts are an
error at `insert()`, and roadmap rule 4 makes silent ambiguity a rule violation, which a bag of independent regexes
structurally cannot fix. The secondary reason is that den's URLPattern binding cannot parse a full pattern string
today, so there is no matcher to compile.

The benchmark, for completeness — and **read the provenance before quoting the numbers.** The benchmark crate
(`/tmp/denplan/R5`) pins `matchit = "0.9.2"` (`Cargo.lock: name = "matchit" version = "0.9.2"`) while this design
pins **0.8.4** everywhere else. It was re-run pinned to `=0.8.4` (`/tmp/denplan/V1/bench84`) and the ranges hold
there too, so the conclusion is version-independent. The figures are also **best-of-N on a loaded machine** (load
average 57 during re-runs): the first re-run gave 22 / 38 / 60 / 8 ns for matchit and 1314 / 13414 / 16081 /
23015 ns for URLPattern — 1.5x to 3x outside the ranges below — and runs 2-3 landed inside them. Treat every
figure here as an **order of magnitude**, not a measurement.

50 routes, 10 000 lookups, release, best of three: matchit 15-16 ns for
`/api/v1/resource0`, 23-26 ns for `/api/v1/users14/9781`, 40-51 ns for a five-segment route, 5-6 ns for a miss
(pinned `=0.8.4`: 17 / 25 / 41 / 5 ns); a URLPattern linear scan over pre-parsed `Url`s: 481-550 ns, 5114-5165 ns,
7137-7343 ns and 7382-7530 ns respectively. Table build, re-measured on the same machine:
`matchit build 50-route table: 279.026us` versus `urlpattern build 50 patterns: 11.452763ms` — **about 40x**, not
the 100x an earlier draft quoted from a single warm run. The scan is structurally penalised because
`UrlPattern::exec` takes `UrlPatternMatchInput` **by value** (an owned `Url`), so N patterns means N owned-Url
allocations per request.

The honest counterweight: at 8.6 us worst case a 50-route URLPattern scan still sustains roughly 116 k req/s of
pure routing, against a real per-request cost of 10-50 us to build a `Request` in QuickJS at all. Routing would be
15-45 % of request cost with URLPattern and about 0.1 % with matchit — meaningful, not existential. **The genuinely
lazy option is to ship `serve({ fetch })` with no route table and let userland dispatch**, which is why routes are
phase 4 and not phase 1.

Rules that must be decided explicitly, because matchit decides none of them:

- Feed it `url.path()` only. A raw request target puts `?x=1` inside a param (`at("/u/7?x=1")` gives
  `id == "7?x=1"`).
- Params are **raw**, not percent-decoded — the same contract as URLPattern's `groups`. Bun's decoding hands
  `../../etc/passwd` to a `/files/:name` handler.
- A `{*rest}` value is passed through verbatim; any future static-file handler owns its own normalise-and-jail.
- Copy params into a fresh null-prototype object **inside the borrow scope**: `matchit::Params<'k,'v>` borrows both
  the router and the path.
- No trailing-slash tolerance and no leading-slash validation are provided; pick a policy and document it.
- A key containing `:` throws with a fix-it naming the brace form.
- Method miss on a matched path is **405 with a computed `Allow`**, never Bun's 404 fallthrough; HEAD is
  synthesised from GET.
- A `Response` route value is pre-rendered at `serve()` and answered without entering QuickJS.

**Known limit, to be documented rather than discovered:** a standalone `{param}` and a standalone `{*catchall}`
cannot be siblings — `matchit` returns `Err(Conflict)` on both 0.8 and 0.9. The classic `/{page}` plus `/{*rest}`
SPA fallback is unrepresentable; the `fetch` fallback covers it, and arguably that is where an SPA fallback belongs.

### 4.6 WebSocket upgrade

101 cannot be a `Response` (fact 14), so it is its own verb returning `{response, socket}` — Deno's shape.
Implementation: `tungstenite::handshake::derive_accept_key` writes the 101 through hyper; `hyper::upgrade::on`
yields the `Upgraded`; one new `pub fn NativeWebSocket::from_upgraded(io, protocol)` over
`WebSocketStream::from_raw_socket(io, Role::Server, None)` feeds den's existing private `run()` loop
(`den-stdlib-networking/src/websocket.rs:509`). Zero new dependencies and no second frame codec.

Upgraded sockets **register with the server handle at the moment of upgrade**. This is not optional: Deno needed an
`ActiveWebSockets` table with graceful and forced modes precisely because upgraded sockets escape the hyper
connection and would otherwise pin shutdown open for ever. `send()` returns the `SendResult` sum type, not Bun's
sentinel integer.

### 4.7 Errors

One `HttpError` class with a `.kind` discriminant drawn from one Rust table, extending the taxonomy 15 §3.7 asks
the fetch client to adopt (`Dns/Connect/Tls/Timeout/Redirect/Body/Aborted`) with the server's
`Bind/AddrInUse/Route/HeadersTimeout/RequestTimeout/IdleTimeout/BodyTooLarge/TooManyConnections/Protocol/Handler`.
A handler throw, a rejected promise and a non-Response return are three *distinct* kinds — Deno collapses the last
two into one indistinguishable `TypeError` at `onError`, and Bun serves a 200 marketing page for the third. The 500
never carries the error text on the wire; the throw is reported through den's existing report sink.

---

## 5. Implementation plan

### Phase 0 — prerequisites in existing crates (not den:http work)

**Files:** `den-stdlib-whatwg/src/streams.rs`, `den-stdlib-whatwg-fetch/src/{headers,lib}.rs`, their tests.

**Deliverable:** (a) fix the teardown abort — take `ctx` from the closure's argument list instead of capturing a
clone at `streams.rs:110`, then grep the tree for every other `Function::new` closure that captures a cloned `Ctx`
**and is itself owned by a JS value**. That last clause is the whole rule and it must be stated precisely, because
the loose version ("reachable from a long-lived JS object") condemns `den-stdlib-wasm`'s `OwnedCtx`
(`backend.rs:98`, also a `Ctx::from_raw` = `JS_DupContext`, reachable from the long-lived per-context `Store`) —
which is *safe*, and which doc 19 §1.4 reuses as its callback re-entry mechanism. The distinction is who releases
it: `rquickjs-core-0.12.2/src/runtime/opaque.rs:284-292` `clear()` drops `spawner` and then `userdata` **before**
`JS_FreeRuntime`, so a cloned `Ctx` held in userdata or captured by a spawned future is dropped in time. Only a
clone held by a **JS value** — a closure QuickJS itself owns, as at `streams.rs:110` — is still alive when the
assertion runs. Grep for the latter; do not "fix" `OwnedCtx`.
(b) delete `is_forbidden_response_header` (`headers.rs:461`) and stop
applying `Guard::Response` in the Response constructor (`lib.rs:434`); keep `Guard::Immutable` (which throws) for
fetch-produced responses, and make any remaining refusal throw rather than return `Ok(())` (`headers.rs:283-285`).

These are pre-existing defects in other crates and belong in their own commits — a new crate's first commit should
not also be carrying a Set-Cookie fix.

**Test:** `echo 'globalThis.s = new ReadableStream({});' | den -` exits 0; a nextest case that constructs a
`ReadableStream`, a `TransformStream` and a `Response` with a stream body, drops the `Engine`, and asserts a clean
shutdown. Plus `new Response('x',{headers:{'set-cookie':'a=1'}}).headers.getSetCookie()` gives `['a=1']`, and the
same after `.append`.

### Phase 1 — the smallest thing that serves a request end to end

**Files:** `Cargo.toml`, `den-stdlib-http/{Cargo.toml,src/{lib,serve,conn,dispatch,bridge,error}.rs}`,
`den-core/{Cargo.toml,src/engine.rs}`, `den-stdlib-whatwg-fetch/src/{request,lib}.rs`,
`den-stdlib-http/tests/serve.rs`, `docs/research/17-graceful-shutdown-and-external-stop.md`.

**Cargo implications, both mandatory:** `stdlib-http` **implies `stdlib-whatwg-fetch`** — `Request`, `Response` and
`Headers` are classes that must be defined in the very context the handler runs in, so `den:http` without them is a
`TypeError` at the first response, not a missing import. And `ConnectionInfo.remote`/`.local` are
`den_stdlib_networking::socket_addr::SocketAddrWrapper` (§3.1: *do not invent an address bag*), so if
`ConnectionInfo` ships in phase 1 then `stdlib-http` also implies `stdlib-networking`. hyper-util features:
`server` + `tokio`, not `server-graceful` (fact 9, fact 16).

**Deliverable:** `serve({ fetch, listen })` giving `Server { addr, url, finished, close() }`. h1 only, cleartext
only, no routes, no TLS, buffered bodies both directions (`DenBody::Full` with
`size_hint == SizeHint::with_exact(len)`; the `Stream` variant exists but is phase 2). **The handler is the full
`(request, connection)` of §4.1 from day one** — `params` is `{}` until phase 4 and `alpn`/`sni`/`peerCertificates`
are `null` until phase 5, but the *arity* is the published one, because shipping `(request)` in phase 1 and widening
in phase 4 breaks every phase-1 handler and contradicts the `.d.ts`. `completed` is real in phase 1 for
`DenBody::Full` (the `Liveness` guard is cheap, §4.1). Synchronous bind. Accept loop and every connection
`ctx.spawn`ed. `close()` flips the `watch`, the accept loop **drops the listener immediately**, `drive()` calls
`graceful_shutdown()` per connection, the alive-token receiver ends, `Phase::Done` is published, the scheduler
empties, `idle()` returns and the process exits **without `exit()`**. `.timer(TokioTimer::new())` on the h1 builder
even though `headersTimeout` is phase 3 — without it hyper's own default is silently off (fact 21). Registered in
the resolver and loader lists only — no `evaluate_def`, so no globals. Includes the two new `pub fn` in
den-stdlib-whatwg-fetch (`Request::from_server`, building headers as a `Headers` **instance** per fact 13 and
building `request.url` from the *listener* per §4.1, and `Response::into_server`), and answers TRACE/CONNECT/TRACK
with 405, GET-with-body with 400, and an absolute-form request target with 400 **in Rust before construction**
(fact 14, §4.1). Every JS throw becomes a 500 via `ctx.catch()`; zero `unwrap()` in `bridge.rs`. **Rewrite
17 §4.2's SIGINT recipe here** (§1.3), scoped: `serve()` drains, a raw `TcpListener` still needs `exit()`.

**Test.** The harness cannot ask the realm anything while `idle()` holds the mutex (fact 8), so `listen: {port: 0}`
resolving the port *inside JS* is unreachable from the test thread. Three ways out; take the third:
(1) a fixed port — flaky in parallel nextest; (2) a Rust-side side channel — the module writes the bound addr into
a `tokio::sync::oneshot` handed in via `Engine::store_userdata`, real but it is test-only machinery in shipping
code; (3) **the JS drives the whole thing itself**, which needs no new seam at all:

```js
const s = serve({ fetch: r => new Response('hi ' + new URL(r.url).pathname), listen: { port: 0 } });
const body = await (await fetch(s.url + '/x')).text();   // den's own client, same realm
globalThis.__result = body;                              // read by the harness after idle() returns
await s.close();                                         // this is what ends the run
```

`fetch` is a `ctx.spawn`ed future like everything else, so it interleaves with the accept loop under the same
`idle()`. The harness then asserts: `__result === 'hi /x'`; `idle()` returned (the run terminated without `exit`);
and — from the harness thread, *after* `idle()` returned — that a connect to `s.addr` is refused. What triggers
`close()` is the script itself, on the line after the assertion; no timer, no handler-side close. That one test
exercises bind, `ctx.spawn` liveness, hyper h1, both bridges, the listener drop and graceful close.

### Phase 2 — streaming bodies, disconnect signal, `completed`

**Files:** `den-stdlib-http/src/{body,bridge,dispatch}.rs`, `den-stdlib-http/tests/body.rs`.

**Deliverable:** incoming body as a native async `pull` on a `ReadableStream` (`TypedArray::new_copy`, never
`ArrayBuffer::new`); `limits.bodyBytes` enforced in Rust before bytes reach JS, giving a typed 413. Outgoing
`DenBody::Stream` over `mpsc::channel(1)` fed by a `ctx.spawn`ed `reader.read()` pump, with `DenBody::Full` keeping
the content-length fast path. The `Liveness` drop-guard yielding both `request.signal` and `conn.completed`.
**Framing is `Body::size_hint`, and does not fall out of the variant** — `Full` returns
`SizeHint::with_exact(len)`, `Stream` returns the default, and without that one method hyper chunks *everything*
(§4.3.5, measured). 204 neither, HEAD headers only. Never touch
`pipeTo`/`pipeThrough`/`tee` — all three are inert or wrong (§3.2).

**Test:** (a) a 2 MB upload consumed chunk by chunk with a per-chunk delay takes about chunks x delay, proving the
body was consumer-paced and not pre-buffered; (b) a 256 KiB-chunk streamed response to a client that stalls 300 ms
parks the producer at a bounded chunk count and peak RSS stays flat — assert *bounded*, not "one chunk ahead"
(fact 5); (c) killing the client mid-body fires `request.signal` with an `AbortError` and rejects
`conn.completed`, and a *successful* response does neither; (d) **a fixed body carries `content-length` and no
`transfer-encoding`** — one `curl -D-` assertion that fails the moment `size_hint` is forgotten.

### Phase 3 — lifecycle: drain deadline, timeouts, typed errors, Disposable

**Files:** `den-stdlib-http/src/{serve,error,dispatch}.rs`, `den-stdlib-http/tests/shutdown.rs`.

**Deliverable:** `close({drainMs})` — stop accepting, FIN idle keep-alives, drain, and at the deadline abort the
survivors' request signals and drop their sockets (the deadline Deno's `poll_fn(poll_complete)` cannot express).
`close({drainMs: 0})` is the forceful path, resolving as soon as sockets are down with every signal already fired
(**not** Bun's `stop(true)`, which still blocks on the abandoned handler); there is no separate `closeNow()`.
`finished` is `phase.subscribe().wait_for(|p| *p == Phase::Done)` on the watch that already exists — one latch.
`{headersTimeout, requestTimeout, idleTimeout, connections, concurrentRequests}` as tokio deadlines with finite
defaults, and `headersTimeout` only works because the h1 builder got `.timer(TokioTimer::new())` in phase 1: set it
without a timer and hyper **panics** inside a `ctx.spawn`ed future holding a live runtime (fact 21). `Symbol.asyncDispose` is `close()`. `onError`, else a 500 that never leaks the error text.

**Test:** a 2 s handler in flight; `close({drainMs:5000})` gives a refused new connect immediately, an idle
keep-alive socket sees EOF, the in-flight request still gets its 200, `finished` resolves after the handler.
Second: `close({drainMs:100})` against a 5 s handler gives an aborted request signal and `finished` at about
100 ms. Third: a socket that sends a partial header block is closed at `headersTimeout` (Deno never closes it).

### Phase 4 — routes as data

**Files:** `den-stdlib-http/src/{router,options,dispatch}.rs`, `Cargo.toml`, `den-stdlib-http/tests/router.rs`.

**Deliverable:** the §4.5 table, compiled once at `serve()`, with every rule in that section enforced. Pre-rendered
static `Response` routes. **No `server.fetch(request)`** — it was cut, see §6.

**Test:** (1) `serve({routes:{"/u/{id}":h,"/u/{name}":h}})` throws `HttpError{kind:"Route"}` naming both keys **and
no socket was bound** (rebinding the port succeeds); (2) `GET /u/7?x=1` gives `params.id === "7"`; (3) `POST` to a
GET-only route gives 405 with `Allow: GET, HEAD`; (4) a key containing `:` throws with the brace fix-it.

### Phase 5 — TLS and h2

**Files:** `den-stdlib-http/src/{tls,conn,options}.rs`, `Cargo.toml`, `den-core/Cargo.toml`,
`den-stdlib-http/tests/tls.rs`.

**Deliverable:** §4.4 in full — ALPN, data-driven SNI, mTLS, `install_default()`, per-connection handshake,
protocol pinned from ALPN. Cleartext `http: "h2c"` behind an explicit option with our own 24-byte preface sniff.

**Test:** with an `rcgen` self-signed cert (already a dev-dependency of den-stdlib-networking): a client offering
`h2,http/1.1` negotiates h2 and completes; one offering only `http/1.1` negotiates h1 and completes; `alpha.test`
and `beta.test` get their own certs and an unknown SNI falls back; with `clientCa` set, a connection without a
client cert is refused and one with a valid cert reaches the handler with `peerCertificates` populated.

### Phase 6 — WebSocket upgrade, and the deletions

**Files:** `den-stdlib-http/src/upgrade.rs`, `den-stdlib-networking/src/websocket.rs`,
`den-stdlib-whatwg/src/{local_http.rs,lib.rs}`, `den-stdlib-whatwg-fetch/src/{lib,body}.rs`,
`den-stdlib-whatwg-fetch/tests/fetch.rs`, `den-stdlib-whatwg/tests/unit/lib.rs`, `ARCHITECTURE.md`,
`den-stdlib-http/tests/websocket.rs`.

**Deliverable:** §4.6. Then delete `den-stdlib-whatwg/src/local_http.rs` (211 lines; its only load-bearing
behaviour, `silent` — accept and never respond — is a den:http handler returning a never-settling Promise) and the
`url.contains("bad-chunk")` test hooks compiled into shipping fetch code (`lib.rs:261, :270`, `body.rs:586`).
Update ARCHITECTURE §1/§3/§9. (17 §4.2's SIGINT recipe was already rewritten in phase 1, where `close()` lands —
leaving it here would have published a recipe that throws the drain away for five phases.)

**Test:** the migration itself is the test — port the existing `local_http`-based cases in
`den-stdlib-whatwg-fetch/tests/fetch.rs` and `den-stdlib-whatwg/tests/unit/lib.rs` onto den:http, which removes a
divergent HTTP/1.1 parser from the test surface. Plus one echo round-trip over an upgraded socket, and one case
asserting `close()` with a live WebSocket completes rather than hanging.

---

## 6. Rejected alternatives

- **A request mailbox (§1.2 seam b).** Correct, and it is den's own worker-port pattern — but the thing it buys is
  thinner than it looks (fact 3 removes the h2 motivation; the drain seam (b) would inherit from hyper-util is
  twelve hand-rolled lines in seam (a), fact 9), and it costs a protocol layer, two hops per request and an extra
  bounded channel on the *request* body that seam (a) does not need at all. Kept on the shelf as the named h2
  fallback (§7 q3).
- **`hyper_util::server::graceful::GracefulShutdown`.** Not a design preference: `GracefulConnection` is not
  implemented for `http1::UpgradeableConnection` (`graceful.rs:166/182/199/217` versus `private::Sealed` at
  `:250`) and the trait is sealed, so the orphan rule closes the door. The only hyper-util type with upgrades and
  graceful is `auto::UpgradeableConnection`, already rejected for `S::Future: 'static`. Replaced by twelve lines
  that serve h1 and h2 identically, so the `server-graceful` feature is never enabled (fact 9).
- **`server.fetch(request)`.** A 15 §3.6 P2 row, cut. The phase-1 test already binds an ephemeral port and drives
  a real socket, so it is not needed to make the server testable; and to exist it must fabricate a
  `ConnectionInfo` — a fake remote and local `SocketAddr`, a fake `alpn`, a `completed` that never sees a
  transport — which is precisely the second dispatch path that phase 4's own "sharing the code, not mirroring it"
  warns against. A handler is a plain function; a script that wants to unit-test one calls it.
- **`closeNow()`.** It is `close({ drainMs: 0 })`. The `serve({signal})` option is rejected two bullets down on
  the grounds that *the handle's own two verbs are the API*; a third verb for an argument value weakens that
  argument and buys no capability.
- **A `LocalSet` on the realm thread (seam c).** Deadlocks against the mutex `idle()` holds, and `spawn_local`
  panics under den's `#[tokio::main]` multi-thread runtime. Deno uses this shape; den cannot copy it.
- **`hyper_util::server::conn::auto::Builder`.** `S::Future: 'static` (fact 10) forbids a service borrowing the
  realm, and its preface read hangs on a silent client.
- **axum / warp / tower.** A second layer over the same hyper for a single handler function.
  `hyper::service::service_fn` is the whole abstraction needed, and axum would drag matchit + tower + tower-http.
- **URLPattern as the route grammar.** No conflict detection, roughly 30-1000x slower per lookup and ~40x slower to build (order of magnitude; §4.5 has the provenance),
  and den's own binding cannot parse a full pattern string (fact 15). URLPattern stays as the WHATWG global and
  should still be finished to spec — that is a different job.
- **`ref()` / `unref()`.** Collides head-on with the liveness mechanism: `ctx.spawn` *is* the ref, so an unref'd
  server could not be a `ctx.spawn`ed future yet must still call into JS. Deno solves it with LocalSet task
  refcounts den does not have. `close()` is the only way to stop a server, which is one fewer concept.
- **`reload()`.** `close()` + `serve()` covers it. Bun's in-place mutation silently drops static routes when
  `routes` is omitted.
- **An `AbortSignal` option on `serve()`.** Deno's is the *forceful* path while its docs describe it as "close the
  server", so a user who wires an AbortController expecting a graceful drain silently kills in-flight requests. The
  handle's own two verbs are the API.
- **`onListen`.** The address is on the handle, synchronously. No callback APIs.
- **A native `CookieMap` in v1.** The actual blocker is the guard (fact 12); deleting three lines makes every
  existing userland cookie library work. A `CookieMap` is a P1 ergonomics row with its own decision.
- **`rustls-pemfile`.** `rustls-pki-types` already covers it (fact 17); this is the difference between six and
  seven new crates.
- **`http-body-util` as a direct dependency.** `DenBody` is hand-written, so `StreamBody`/`Full`/`Empty` are
  unused; the one useful item, `BodyExt::frame`, is a three-line `poll_fn`. `Bytes`, `Frame`, `StatusCode` and the
  `Body` trait all come through hyper's re-exports.
- **A second WebSocket implementation inside den:http.** den already has about 750 lines that work (fact 18).
- **`(req, res)`, `node:cluster`, filesystem routing, h2 server push, HTML entrypoint routes, an in-server
  bundler.** All six are 15 §6 "deliberately not", with reasons already recorded there.

---

## 7. Open questions / limits

1. **`completed` for cheap bodies — RESOLVED: unconditional.** The question was whether making `completed`
   truthful for a 5-byte string response costs a frame-completion path that a fixed body would otherwise skip. It
   does not, because `DenBody` is hand-written (§6 rejects `http-body-util`): hyper polls `poll_frame` on *every*
   body, `Full` included, so the terminal-`None` disarm is already on the path. The whole incremental cost of a
   truthful `completed` on a fixed body is one `oneshot::channel` allocation and one `Drop` impl per request. So
   the `.d.ts` declares it unconditionally, phase 1 implements it for `Full` and phase 2 for `Stream`, and Deno's
   answer (promise the strong thing, resolve optimistically before the write, never reject) stays rejected.
2. **Two param grammars ship in one runtime**: den:http routes use `{id}` while the URLPattern global uses `:id`.
   15 §5.2 (line 566) wanted one grammar. This is a real product cost, taken because fact 15 leaves no alternative
   today. Finishing URLPattern to spec and *then* compiling its matcher into the router remains the better end
   state and is not scheduled here.
3. **h2 under load and with streamed bodies. The `idle()` teardown half is discharged.** An earlier draft feared
   "a connection future parked mid-poll across an `idle()` teardown". That failure mode is **impossible on one
   thread**: doc 16 §3.2 establishes that the scheduler drive is synchronous under the runtime lock, so there is
   no mid-poll moment for a drop to land in, and a wake arriving in a drop window is not lost —
   `schedular.rs:145` re-registers the waker and `:152` pops from the intrusive `should_poll` queue on the next
   poll. Then it was run: CHURN mode in `/tmp/denplan/V1/probe` drops and recreates `idle()` every 7 ms while
   serving. `[V1] idle() dropped and recreated 74 times while serving` (h1, in-flight async JS handler, graceful
   drain, `rc=0`) and `[V1] idle() dropped and recreated 28 times while serving` +
   `[V1] curl h2 -> status 200 ver 2` (h2 via `LocalExec`). What remains genuinely untested is **h2 under
   concurrent load and with streamed bodies** — write that test before trusting it. Fallback if it fails:
   advertise only `http/1.1` in ALPN and reject the cleartext preface with `HttpError{kind:"Protocol"}` — an
   explicit failure, never a silent downgrade.
4. **`hyper::rt::Executor` being `Send`-free is a blanket-impl property, not an advertised guarantee.** If hyper
   1.x ever tightens `Http2ServerConnExec`, the h2 path breaks. Low probability (the trait is sealed and stable
   since 1.0); pin hyper with a caret and keep h1-only as a standing fallback.
5. **`NetBind` is reserved and unimplemented.** 15 §3.19 rates a permission system P0 and §5.2 says `serve()`
   should take the bind capability as a *value* so an untrusted module importing den:http still cannot bind. The
   options type reserves the field now because adding it later breaks every call site; nothing in this plan makes
   it land.
6. **One cargo feature or two — RESOLVED: two, and it ships split.** Folding TLS into `stdlib-http` is not a
   neutral deferral, because `stdlib-http` joins the `stdlib` aggregate (§4.2) and `den-core/Cargo.toml:41` is
   `default = ["stdlib", "typescript", "react", "wasm", "wasi", "jit"]`. `cargo tree -i` confirms rustls,
   rustls-webpki, ring, untrusted, subtle and tokio-rustls are all absent today, so folding them in means +6
   crates and the measured +1.58 MiB stripped (3 434 696 bytes with rustls versus 1 853 256 without, same probe
   crate) on **every default `cargo build` and every downstream embedder**. 15 §4.7 — which this document claims
   to honour — says the opposite in as many words: *"Heavy backends (rustls, hickory, ICU4X, brotli/zstd, argon2)
   join as feature packs."* The split is three lines of `Cargo.toml` now and an un-shipping later.
7. **Two TLS stacks.** native-tls (dynamically linked, outbound) plus rustls (inbound) means two trust stories and
   two error taxonomies to map into `HttpError`. Acceptable for one release; real debt beyond that. A full client
   migration touches nine files, flips reqwest and tokio-tungstenite feature sets, and needs a trust-store crate
   (rustls-native-certs or rustls-platform-verifier) that native-tls provides for free.
8. **The client and the server will diverge unless 15 §3.7's six couplings are fixed together.** This plan fixes
   exactly one (the header guard). `fetch` still drains request bodies into a `Vec` before sending
   (`fetch_op.rs:1062`, so `duplex:'half'` is accepted and ignored), still pre-buffers responses under an 8 MB
   heuristic (`:1636`), and `clone()` only works for buffered bodies. Ship this and den has **two** stream cores,
   which is what 15 §4.3 exists to prevent.
9. **den:streams stays broken.** `pipeThrough` never pumps, `pipeTo` buffers everything and never closes the sink,
   `tee`/`from`/async-iteration do not exist in JS. den:http correctly touches none of them — and therefore
   compression, SSE and any middleware that reads a body twice have no transform to build on. Separate, larger job.
10. **Not shipped, and the §3.6 better-than column still wants them:** compression, static files with Range/ETag,
    `CookieMap`, signed cookies, SSE, HTTP/3, mTLS beyond a boolean requirement, systemd socket activation,
    metrics, tracing. `serveDir` in particular must not be built before the read capability it is meant to be
    scoped by exists — a `{*filepath}` catch-all hands `../../etc/passwd` through verbatim.
11. **`Uint8Array` became generic in TS 5.7.** The `.d.ts` above uses `Uint8Array<ArrayBuffer>` throughout; anything
    less produces TS2322 for callers who annotate. `const` type parameters need TS >= 5.0 and `await using` needs
    TS >= 5.2 plus `ESNext.Disposable` in `lib`. `examples/tsconfig.json` today produces 18 errors, 12 of them from
    two missing flags; fix that before adding a new `.d.ts` to it.
12. **A raw `TcpListener` still has no close handle.** §1.3: this document answers half of 17 §7 q7.
    `den-stdlib-networking/src/socket.rs:59-69` gives `TcpListenerWrapper` only `local_addr`, `accept(self)` and
    the static `listen`. Until it gets a `close()`/`Symbol.dispose` of the same three-line `watch` shape, the
    rewritten 17 §4.2 has to carry two recipes: drain for `serve()`, mandatory `exit()` for a hand-rolled accept
    loop. Small, unscheduled, and a live inconsistency in the shutdown story.
13. **The TLS handshake cost on the realm thread is unmeasured.** §4.2 moves the handshake out of the accept loop,
    which fixes the head-of-line DoS of `tls.rs:93-98` — but it lands it on the realm thread under the runtime
    mutex, where a ~1 ms asymmetric operation stalls every timer, every `fetch` and every other connection, and on
    a worker realm (`worker_threads(1)`, ARCHITECTURE §7.3) there is no second thread at all. Fact 20's isolation
    warning is about handlers; this is the same warning about den's own code, and nobody has put a number on it.
    If it turns out to matter, the fix is the one thing seam (b) is genuinely good at (§1.2) — hand the handshake
    to tokio and take the `Send` hop only there, via `LazyConfigAcceptor` (§4.4), not the whole protocol layer.
14. **No `trustedProxy`.** §4.1 pins `request.url` to the listener's own address and ignores `Host`, `:authority`,
    `Forwarded` and every `X-Forwarded-*`. That is the right default and the wrong answer for den behind a
    TLS-terminating proxy, which is where most servers live. The eventual shape is an explicit
    `trustedProxy: { hops, scheme?, authority? }` on `ServeOptions` so the trust is a declared value rather than
    an ambient assumption — not designed here, and deliberately not a "just read the header" default.
15. **Windows and macOS are unprobed.** Every measurement here is Linux x86_64. The `Send` analysis is a
    type-system fact and holds everywhere; the socket-buffer number in fact 5 does not.

---

## Probe directories

| Dir | What |
|---|---|
| `/tmp/denplan/writer/` | `s1.js`/`s2.js`/`s3.js` and `t.js` (stream teardown reachability matrix on `target/debug/den` and `target/min-size-release/den`), `c.js` (header guards, URLPattern, forbidden method, GET+body, status 101, `tee`/asyncIterator), `w-probe.log`, `r1-rerun.log` |
| `/tmp/denplan/writer/probe/` | `src/main.rs` — **W1** h1 keep-alive with a `!Send` service capturing `Function<'js>`, **W2** async JS handler plus `graceful_shutdown` drain of an in-flight request (deterministic: the service signals in-flight), **W3** h2 over `LocalExec(Ctx)` driven by `curl --http2-prior-knowledge`; `src/bin/neg.rs` (negative: a `Body` borrowing `'js` rejected by `S::ResBody: 'static`); `src/bin/bp.rs` (**W4** `mpsc::channel(1)` response body against a stalled client, 7-byte and 256 KiB chunks) |
| `/tmp/denplan/r1-realm-threading/probe/` | the original three-claim probe (`claim1` tokio task does not ref, `claim2` `ctx.spawn` does, `claim3` graceful drain); `src/bin/auto.rs` (`auto::Builder` `S::Future: 'static` failure), `src/bin/neg.rs`. Its `claim3` in-flight assertion races on `curl` startup and reproduced empty on re-run — W2 is the deterministic replacement |
| `/tmp/denplan/R2/` | Deno 2.9.4: `deno.d.ts`, `p1_errors.ts`, `p1b_onerror.ts`, `p2_server.ts`, `p3`/`p3b` (legacy abort), `p4_shutdown.ts`, `p5_life.ts`, `p6_upstream.ts`, `p7_ws.ts`, `p9_perm.ts`, `p10_misc.ts`, `p11_conc.ts` and their logs |
| `/tmp/denplan/R3/` | Bun 1.3.9: `serve.d.ts`, `p1_routing.ts`, `p2_edge.ts`, `p3_reload.ts`/`p3b.ts`, `p4_ws_backpressure.ts`/`p4b_ws_limit.ts`, `p5_stop.ts`, `p6_signal.ts`, `p7_cookies_error.ts`, `p8_tls_unix.ts`, `p9_misc.ts`, `p10_timeout.ts`, `p11_conflict.ts`; `mt/` (matchit semantics) |
| `/tmp/denplan/R4/` | hyper plus rustls: `src/main.rs` (h1/h2 over tokio-rustls with ALPN, streamed body, `PemObject` instead of rustls-pemfile), `src/bin/{notls,autosniff}.rs`, `probe.stripped`/`notls.stripped` (binary-size delta), `ringonly/` and `awslc/` (provider build-time comparison), logs |
| `/tmp/denplan/R5/`, `/tmp/denplan/R5b/` | matchit versus URLPattern: `src/main.rs` (50-route, 10 000-lookup benchmark, conflict probe), `src/bin/setup.rs` (table build), `src/bin/{borrow,edge,traversal}.rs` (params borrow both router and path; query, percent and dot-segment behaviour) |
| `/tmp/denplan/r6/` | den-side reuse: `req.js`, `hdr.js`, `resp2.js`, `p1.js`, `p2.js` (Request/Response/Headers construction from Rust, `pull` backpressure, `pipeThrough`/`pipeTo`/`tee` state) |
| `/tmp/denplan/V1/` | **The verification pass that produced facts 9, 21 and the §4.2 rewrite.** `probe/src/bin/neg_upgrade.rs` (compile-refutes `GracefulShutdown::watch` on `http1::UpgradeableConnection`), `probe/src/bin/timer_panic.rs` (the `header_read_timeout` panic, `rc=101`), `probe/src/main.rs` in `UPG=1` mode (the hand-rolled drain: `[V1] drain completed`, in-flight body preserved, `rc=0`), in `CHURN` mode (74 h1 / 28 h2 `idle()` drop-and-recreate cycles while serving, §7 q3), and the `size_hint` framing comparison; `bench84/` (§4.5 re-run pinned to `matchit = "=0.8.4"`) |
