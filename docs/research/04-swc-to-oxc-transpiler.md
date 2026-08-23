# 04 — Replacing den's swc transpiler with oxc 0.146

Status: researched + **compiled + tested + independently re-verified**. Every snippet below was
built and run against the real crates (`oxc_* 0.146.0`, `oxc_sourcemap 8.1.2`); all 4 feature
combinations (`transpile`, `+typescript`, `+react`, `+typescript,react`) compile with zero warnings
and the §12 suite is green in all four. §9's code was additionally dropped into a clean copy of the
den working tree and `cargo check -p den-core` run — it contributes zero errors and zero warnings
(§10.1 lists what *does* still fail, and why none of it is this migration's fault). Line references
are to files actually read; see the verification log at the end for what was re-checked and when.

---

## 0. Where we are today

`den-transpiler-oxc` is a **rename only** — the code in
`/home/steve/git/github.com/stevefan1999-personal/den/den-transpiler-oxc/src/lib.rs` is still swc, and
`den-core` still does `use den_transpiler_swc::{...}` in three files. **The workspace does not build
right now.** The rename half of this job is unavoidable regardless of the transpiler swap.

The public surface that must survive (all four items are load-bearing):

| Item | Used by |
|---|---|
| `EasySwcTranspiler::default()` + `.transpile(&str, Syntax, IsModule, bool) -> Result<(String, Option<SourceMap>), E>` | `engine.rs:341`, `loader/http.rs:83`, `loader/mmap_script.rs:61` |
| `infer_transpile_syntax_by_extension(&str) -> Option<Syntax>` (`Syntax: Default`, callers use `.unwrap_or_default()`) | `engine.rs:350`, `http.rs:85`, `mmap_script.rs:63` |
| `get_best_transpiling() -> &'static str` (const fn) | `engine.rs:350` |
| `IsModule::{Bool(bool), Unknown}`, `SourceMap`, `Syntax`, `EasySwcTranspilerError`, `InferTranspileSyntaxError` | imported at `engine.rs:12-18` (`use { den_transpiler_swc::{…}, std::sync::Arc }`), `EngineError` at `engine.rs:381-390` (`EasySwcTranspiler` variant `:384`, `InferTranspileSyntaxError` `:389`) |

**Grep result that decides §3:** the sourcemap is *never consumed*. All three call sites pass
`emit_sourcemap = false` and destructure with `let (src, _) = ...`. `SourceMap` appears only in the
`Engine::transpile` return type (`engine.rs:340`). Nothing calls `to_json_string`, `lookup_token`, or
anything else on it.

Also worth knowing: `Arc<EasyOxcTranspiler>` is a **required** bound, not incidental —
`HttpLoader`/`MmapScriptLoader` use `#[derivative(Default(new = "true"))]`, and `Arc<T>: Default`
requires `T: Default`. So keep `#[derive(Default)]` on the transpiler struct even though it is a ZST.

---

## 1. The pipeline (real 0.146 signatures)

Canonical order, from
`/home/steve/git/github.com/oxc-project/oxc/crates/oxc_transformer/examples/transformer.rs:52-101`:

```
Allocator  →  SourceType  →  Parser::new(&alloc, src, st).parse()
           →  SemanticBuilder::…build(&program).semantic.into_scoping()
           →  Transformer::new(&alloc, path, &opts).build_with_scoping(scoping, &mut program)
           →  Codegen::new().with_options(o).build(&program)
```

Signatures verified in the registry copies:

```rust
// oxc_parser-0.146.0/src/lib.rs:266,281,361
pub struct Parser<'a, C: ParserConfig = NoTokensParserConfig> { .. }
impl<'a> Parser<'a> {
  pub fn new(allocator: &'a Allocator, source_text: &'a str, source_type: SourceType) -> Self;
}
impl<'a, C: ParserConfig> Parser<'a, C> { pub fn parse(self) -> ParserReturn<'a>; }

// oxc_parser-0.146.0/src/lib.rs:149-193  (#[non_exhaustive])
pub struct ParserReturn<'a> {
  pub program: Program<'a>,
  pub module_record: ModuleRecord<'a>,
  pub diagnostics: Diagnostics,        // NOT Vec<OxcDiagnostic> — see §5
  pub irregular_whitespaces: Box<[Span]>,
  pub tokens: ArenaVec<'a, Token>,
  pub panicked: bool,
  pub is_flow_language: bool,
}

// oxc_semantic-0.146.0/src/builder.rs:131-135, 191, 248, 303, 311
pub struct SemanticBuilderReturn<'a> { pub semantic: Semantic<'a>, pub diagnostics: Diagnostics }
impl SemanticBuilder<'_> {
  pub fn new_compiler() -> Self;                    // build_nodes=false, check_syntax_error=true
  pub fn with_enum_eval(mut self, yes: bool) -> Self;
  pub fn with_excess_capacity(mut self, excess_capacity: f64) -> Self;
  pub fn build(mut self, program: &'a Program<'a>) -> SemanticBuilderReturn<'a>;
}
// oxc_semantic-0.146.0/src/lib.rs:112
impl<'a> Semantic<'a> { pub fn into_scoping(self) -> Scoping; }   // Scoping has NO lifetime

// oxc_transformer-0.146.0/src/lib.rs:117,132
impl<'a> Transformer<'a> {
  pub fn new(allocator: &'a Allocator, source_path: &Path, options: &TransformOptions) -> Self;
  pub fn build_with_scoping(self, scoping: Scoping, program: &mut Program<'a>) -> TransformerReturn;
}
// oxc_transformer-0.146.0/src/lib.rs:87-96  (#[non_exhaustive]; `helpers_used` is #[deprecated] —
// do NOT destructure this struct, use field access)
pub struct TransformerReturn { pub diagnostics: Diagnostics, pub scoping: Scoping, .. }

// oxc_codegen-0.146.0/src/lib.rs:51-68, 175, 209, 256   (#[non_exhaustive] — a struct pattern
// must end in `..`, which §9's `let CodegenReturn { code, map, .. }` does)
pub struct CodegenReturn<'a> {
  pub code: String,
  #[cfg(feature = "sourcemap")] pub map: Option<oxc_sourcemap::SourceMap<'a>>,   // ← borrowed!
  pub legal_comments: Vec<Comment>,
}
impl<'a> Codegen<'a> {
  pub fn new() -> Self;
  pub fn with_options(mut self, options: CodegenOptions) -> Self;
  pub fn build(mut self, program: &Program<'a>) -> CodegenReturn<'a>;
}
```

### 1.1 `Scoping` is the borrow-checker escape hatch

`SemanticBuilder::build` takes `&'a Program<'a>` (shared borrow), but
`Transformer::build_with_scoping` needs `&mut Program<'a>`. This only works because
`Semantic::into_scoping()` returns a **lifetime-free** `Scoping`
(`oxc_semantic-0.146.0/src/lib.rs:112`), which ends the shared borrow. Sequence must be:

```rust
let semantic = SemanticBuilder::new_compiler()… .build(&program);   // & borrow
if semantic.diagnostics.has_errors() { … }                          // still borrowed
let scoping = semantic.semantic.into_scoping();                     // borrow ends here
Transformer::new(&allocator, path, &opts).build_with_scoping(scoping, &mut program);
```

### 1.2 🔴 `with_enum_eval(true)` is mandatory — otherwise the transformer **panics**

Confirmed by running it. `SemanticBuilder::new_compiler()` alone gives
`enum_eval = false`, and the TS enum transform asserts:

```
thread 'enum_and_namespace' panicked at oxc_transformer-0.146.0/src/typescript/enum.rs:171:9:
Transformer requires `Scoping` produced with `SemanticBuilder::with_enum_eval(true)`
to correctly transform `enum E`.
```

Any user TS file containing `enum` would abort the whole `den` process. The official
`transformer.rs` example has `.with_enum_eval(true)` at line 74 for exactly this reason. **Do not
drop it.**

---

## 2. TypeScript stripping + JSX config

### `TransformOptions` — field names from `oxc_transformer-0.146.0/src/options/mod.rs:39-85`

```rust
pub struct TransformOptions {
  pub cwd: PathBuf,
  pub assumptions: CompilerAssumptions,
  pub typescript: TypeScriptOptions,   // ← TS stripping
  pub decorator: DecoratorOptions,
  pub plugins: PluginsOptions,
  pub jsx: JsxOptions,                 // ← JSX
  pub env: EnvOptions,                 // ← downleveling; Default = everything OFF
  pub proposals: ProposalOptions,
  pub helper_loader: HelperLoaderOptions,
}
```

`#[derive(Debug, Default, Clone)]` — so `TransformOptions::default()` is exactly "strip types, do
JSX, downlevel nothing". `EnvOptions` is `#[derive(Default)]` over per-edition option structs
(`options/env.rs:25-...`) whose defaults are all `false`/`None`, and `Module::default() == Preserve`
(`options/module.rs:16-19`) so ESM is **not** rewritten to CJS. That is what den wants.

> Do **not** use `TransformOptions::enable_all()` (the example's fallback). It turns on
> `development: true`, react-refresh, styled-components, legacy decorators and the whole
> `EnvOptions::enable_all()` downlevel set (`options/mod.rs:92-...`).

### TypeScript — `oxc_transformer-0.146.0/src/typescript/options.rs:20-...`

```rust
pub struct TypeScriptOptions {
  pub jsx_pragma: Cow<'static, str>,        // default "React.createElement"
  pub jsx_pragma_frag: Cow<'static, str>,   // default "React.Fragment"
  pub only_remove_type_imports: bool,       // default false
  pub allow_namespaces: bool,               // default TRUE
  pub allow_declare_fields: bool,           // default TRUE  (deprecated, but load-bearing, see below)
  pub remove_class_fields_without_initializer: bool, // default false
  pub optimize_const_enums: bool,           // default FALSE → `const enum` keeps a runtime IIFE
  pub optimize_enums: bool,                 // default false
  pub rewrite_import_extensions: Option<RewriteExtensionsMode>,  // default None
}
```

`TypeScriptOptions::default()` is the right choice, and it is the **direct equivalent of den's old
swc `native_class_properties = true`**. Why: `lib.rs:167-170` gates the ES2022 class-properties
downlevel on

```rust
x2_es2022: ES2022::new(self.env.es2022,
  !self.typescript.allow_declare_fields || self.typescript.remove_class_fields_without_initializer)
```

With the defaults that expression is `false` and `env.es2022.class_properties` is `None`, so class
fields are emitted natively (`lib.rs:174-177`). Verified:

```
in : class A { x = 1; declare y: number; z!: string; }
out: class A {\n\tx = 1;\n\tz;\n}
```

There is **no** "TS only, isolatedModules" toggle to find — TypeScript stripping happens because
`SourceType::is_typescript()` is true, gated at `oxc_transformer-0.146.0/src/lib.rs:163-166`:

```rust
x0_typescript: program.source_type.is_typescript()
    .then(|| TypeScript::new(&self.typescript, &self.state)),
```

So *"TS on/off" is a property of the `SourceType`, not of the options*. The `typescript` cargo
feature therefore controls `infer_transpile_syntax_by_extension` (whether `.ts` maps to a
TS `SourceType` at all), not `TransformOptions`.

### JSX — `oxc_transformer-0.146.0/src/jsx/options.rs:37-...`

```rust
pub struct JsxOptions {
  pub jsx_plugin: bool, pub display_name_plugin: bool,
  pub jsx_self_plugin: bool, pub jsx_source_plugin: bool,
  pub runtime: JsxRuntime,              // Classic | Automatic ; #[default] = Automatic
  pub development: bool,                // default false
  pub throw_if_namespace: bool,         // default true
  pub pure: bool,                       // default true → emits /* @__PURE__ */
  pub import_source: Option<String>,    // automatic runtime; default "react"
  pub pragma: Option<String>,           // classic runtime; default "React.createElement"
  pub pragma_frag: Option<String>,      // classic runtime; default "React.Fragment"
  pub use_built_ins: Option<bool>, pub use_spread: Option<bool>,
  pub refresh: Option<ReactRefreshOptions>,
}
impl Default for JsxOptions { fn default() -> Self { Self::enable() } }   // options.rs:118-121
pub fn enable()  -> Self;   // options.rs:135 — jsx_plugin=true, display_name_plugin=true, Automatic
pub fn disable() -> Self;   // options.rs:155 — everything off
```

🔴 **`JsxOptions::default()` is wrong for den.** Default runtime is `Automatic`
(`options.rs:14-19`, `#[default] Automatic`), which rewrites `<div/>` into
`import { jsx as _jsx } from "react/jsx-runtime"`. den's module resolver has no `react`, so every
JSX file would fail to load at runtime.

swc's default was `Classic` — `swc_ecma_transforms_react-6.0.0/src/jsx/mod.rs:39-43`:
`impl Default for Runtime { fn default() -> Self { Runtime::Classic } }`, and den passed
`Default::default()` at old `lib.rs:112`. So for behaviour parity use:

```rust
JsxOptions { runtime: JsxRuntime::Classic, ..JsxOptions::enable() }
```

which produces `React.createElement(...)` exactly as before (verified:
`const a = /* @__PURE__ */ React.createElement("div", { x: 1 }, "hi");`).

When the `react` feature is **off**, use `JsxOptions::disable()`. The JSX plugin itself would be a
no-op (a non-JSX `SourceType` never produces JSX nodes), but `display_name_plugin` also rewrites
`createReactClass({...})` calls in plain JS, so disabling is the honest thing.

`@jsx` / `@jsxRuntime` pragma comments are still honoured — `lib.rs:147-157` calls
`jsx::update_options_with_comments` on leading comments when `source_type.is_jsx()`, same as swc did.

Verified classic-runtime output for the awkward cases (spread, fragment, child spread, namespaced
attribute) — all plain `React.createElement`, no helper import:

```
<div {...props} k={1}/>  →  React.createElement("div", { ...props, k: 1 })
<><b/><i/></>            →  React.createElement(React.Fragment, null, …)
<div>{...xs}</div>       →  React.createElement("div", null, ...xs)
<a xlink:href="x"/>      →  React.createElement("a", { "xlink:href": "x" })
```

### Helpers: nothing to configure — **as long as decorators stay off**

`HelperLoaderOptions::default()` is `mode: Runtime, module_name: "@oxc-project/runtime"`
(`common/helper_loader.rs:120-137`), which would inject unresolvable imports. With the options in
this document it never fires: `grep -rn "helper_call\|helper_load\|Helper::" src/typescript/
src/jsx/` in oxc_transformer returns **nothing**, and every `es20xx` transform is off via
`EnvOptions::default()`. Leave it alone; do not copy the example's `HelperLoaderMode::External`.

The one thing that *does* wake the helper loader is the decorator transform — see next section.

### 🔴 Decorators — an undocumented behaviour regression, decide deliberately

`DecoratorOptions::default()` is `legacy: false`, and oxc's parser accepts decorator syntax in TS
unconditionally. Result: **`@dec class A {}` parses, is left completely untransformed, and is
emitted verbatim.** QuickJS does not implement decorators, so den fails *later*, at
`Module::declare`, with a raw `SyntaxError` instead of a transpile error.

swc behaved differently: `TsSyntax::decorators` defaults to `false`
(`swc_ecma_parser-6.0.0/src/lib.rs:316`) and den passed `Syntax::Typescript(Default::default())`,
so decorator syntax was a **parse error** at transpile time with a clear message.

Measured, both branches:

```
in : @dec class A { @dec2 m() {} }          (ts)

DecoratorOptions::default()      → "@dec class A {\n\t@dec2 m() {}\n}"        ← invalid for QuickJS
DecoratorOptions{legacy:true,..} → import _decorate from "@oxc-project/runtime/helpers/decorate";
                                   let A = class A { m() {} };
                                   _decorate([dec2], A.prototype, "m", null);
                                   A = _decorate([dec], A);                   ← unresolvable import
```

So **neither** default gives working output. Pick one and write it down:

- **Ship as-is** (`DecoratorOptions::default()`, what §9 does). Zero code, zero deps; decorator
  users get a confusing downstream error. Acceptable because den never supported decorators.
- **Reject early** — one guard after parsing keeps swc's diagnostic quality:
  `if program.body.iter().any(|stmt| /* class with non-empty decorators */)` → `Err(Transform(..))`.
- **Actually support them** — requires `legacy: true` *and* shipping `@oxc-project/runtime` in the
  module resolver, or `HelperLoaderMode::Inline`. Out of scope for this migration.

---

## 3. Source maps — switch the public type, don't convert

### The facts

`CodegenReturn<'a>.map` is `Option<oxc_sourcemap::SourceMap<'a>>`
(`oxc_codegen-0.146.0/src/lib.rs:59`) and it **borrows the arena and the source text**.
`oxc_codegen-0.146.0/src/sourcemap_builder.rs:84-99`:

```rust
pub fn into_sourcemap(mut self) -> oxc_sourcemap::SourceMap<'a> {
    oxc_sourcemap::SourceMap::from_parts(SourceMapParts {
        names: self.names.into_iter().map(Cow::Borrowed).collect(),     // ← &'a str from arena
        sources: vec![Cow::Owned(self.source_name)],
        source_contents: vec![Some(Cow::Borrowed(self.original_source))], // ← &'a str
        …})
}
```

So it **cannot** escape the `Allocator` scope without `SourceMap::into_owned() -> SourceMap<'static>`
(`oxc_sourcemap-8.1.2/src/sourcemap.rs:102-117`). `oxc_sourcemap` also ships
`OwnedSourceMap` — a newtype over `SourceMap<'static>` that exists precisely so downstream code can
avoid the `'static` annotation (`oxc_sourcemap-8.1.2/src/owned_sourcemap.rs:36`, `new` at `:43`).

### Decision: `pub use oxc_sourcemap::OwnedSourceMap as SourceMap;`

Justification, in order of weight:

1. **den never reads the sourcemap.** Three call sites, all `emit_sourcemap = false`, all discarding
   with `let (src, _)`. Converting to the `sourcemap` crate would mean serialising to JSON and
   re-parsing (there is no structural bridge) — real CPU, zero consumers.
2. `OwnedSourceMap` has no lifetime parameter, so `Option<SourceMap>` in `Engine::transpile`'s
   signature (`engine.rs:340`) stays byte-identical. Zero churn in `den-core`.
3. The `sourcemap` crate dependency disappears entirely.

Conversion cost at the boundary is one call: `map.into_owned()` then `OwnedSourceMap::new(..)`.

`CodegenOptions::source_map_path: Option<PathBuf>` (`oxc_codegen-0.146.0/src/options.rs:29`) is the
on/off switch — `Some(path)` ⇒ map produced, and `path` lands in `sources[0]`. Verified output for
`emit_sourcemap = true`:

```json
{"version":3,"names":[],"sources":["<anonymous>"],
 "sourcesContent":["const x: number = 1; export {};"],"mappings":"AAAA,MAAM,IAAY;AAAG"}
```

> Even lazier option, if you want it: delete the `emit_sourcemap` param and the `Option<SourceMap>`
> from the return type. Nothing in the repo would notice. Not proposed here because the brief says
> preserve the API — but flag it, it's ~15 lines of dead surface.

---

## 4. `SourceType` from an extension, and `IsModule`

`oxc_span-0.146.0/src/source_type.rs`. `SourceType` fields are `pub(super)` (lines 24-36) so you must
go through constructors/builders. `Default` is `Self::mjs()` (line 96-100), which keeps
`.unwrap_or_default()` at den's call sites meaningful.

Constructors (all `const fn`): `script()` (278), `mjs()` (258), `cjs()`, `unambiguous()`, `jsx()`
(315), `ts()` (339), `tsx()` (363), `d_ts()` (379). Builders: `with_script` (463) /`with_module`
(475) /`with_unambiguous` (487) /`with_commonjs`/`with_javascript`/`with_typescript`/`with_jsx`/
`with_standard` (559), each a `#[must_use] const fn(self, yes: bool)` that is a **no-op when
`yes == false`**.

`SourceType::from_extension(&str)` (line 639) accepts exactly
`VALID_EXTENSIONS = ["js","mjs","cjs","jsx","ts","mts","cts","tsx"]` (line 119). Measured mapping:

| ext | `from_extension` result | notes |
|---|---|---|
| `js` | JavaScript / **Unambiguous** / Standard | content-detected |
| `mjs` | JavaScript / Module / Standard | |
| `cjs` | JavaScript / CommonJS / Standard | |
| `jsx` | JavaScript / Unambiguous / **Jsx** | |
| `ts` | TypeScript / **Unambiguous** / Standard | |
| `mts` | TypeScript / Module / Standard | |
| `cts` | TypeScript / CommonJS / Standard | |
| `tsx` | TypeScript / Unambiguous / **Jsx** | |
| `mjsx` | ❌ `Err(UnknownExtension)` | den registers this — hand-build `SourceType::jsx()` |

`cjs`/`mts`/`cts` are newly *accepted* by §9's `infer_transpile_syntax_by_extension` (the swc version
had no arm for them), but they are unreachable in practice: `engine.rs:201-217` registers only
`js`, `mjs`, `jsx`, `mjsx`, `ts`, `tsx` with the mmap loader, and `http.rs` derives only `"js"` or
`"ts"` from the MIME type. No behaviour change, just a smaller `match`.

### `IsModule` → `ModuleKind`, including `Unknown`

**oxc has a first-class heuristic; no need to invent one.** `ModuleKind::Unambiguous`
(`source_type.rs:60-68`) is documented as *"Consider the file a module if ESM syntax is present, or
else consider it a script"*, and the parser resolves it in place at
`oxc_parser-0.146.0/src/lib.rs:736-...`:

```rust
if source_type.is_unambiguous() {
    if module_record.has_module_syntax { program.source_type = source_type.with_module(true); }
    else                               { program.source_type = source_type.with_script(true);  }
}
```

Top-level `await` also upgrades to ESM (`oxc_parser-0.146.0/src/js/expression.rs:1722-1734`,
"upgrade to ESM immediately (like Babel's `sawUnambiguousESM`)"), which matters because den's REPL
path (`engine.rs:351-355`) transpiles with `IsModule::Unknown`. Verified: `await import(\`./x.js\`)`
round-trips cleanly under `Unknown`.

Mapping:

```rust
IsModule::Bool(true)  => syntax.with_module(true)
IsModule::Bool(false) => syntax.with_script(true)
IsModule::Unknown     => syntax.with_unambiguous(true)
```

Note swc's `IsModule::default()` is `Bool(true)` (`swc_config-1.0.0/src/module.rs:16-20`). den never
calls it, so the new enum simply doesn't implement `Default` — one fewer thing to get wrong.

---

## 5. Diagnostics

No `Handler`, no tty emitter, no `Globals`/`GLOBALS` thread-local. Each stage returns
`oxc_diagnostics::Diagnostics` — a `Vec<OxcDiagnostic>` newtype with `Deref`, `has_errors()`,
`has_warnings()`, `errors()`, `warnings()`, `into_vec()` (`oxc_diagnostics-0.146.0/src/lib.rs:92-130`).
Note it is `Diagnostics`, **not** `Vec<OxcDiagnostic>` — a `.iter().any()` over the raw vec would
treat warnings as errors. It also implements `IntoIterator<Item = OxcDiagnostic>` (`lib.rs:169`),
which is what lets §9's `render(source, diagnostics)` take `impl IntoIterator<Item = OxcDiagnostic>`
and accept a `Diagnostics` by value.

Rendering (`oxc_diagnostics-0.146.0/src/lib.rs:308,487`):

```rust
impl OxcDiagnostic {
    pub fn render(&self) -> String;                                     // no source snippet
    pub fn render_with_source_code<T: SourceCode>(self, source_code: T) -> String;
}
```

`&str`, `String`, `Arc<T>` all implement `SourceCode` (`source_impls.rs:767-786`), and
`NamedSource::new(name, source)` (`named_source.rs:23-30`) attaches a filename. `NamedSource` and
`SourceCode` are re-exported from the crate root (`lib.rs:61-65`), so `use oxc_diagnostics::
{NamedSource, OxcDiagnostic}` is enough. Both `render*` methods internally use
`GraphicalReportHandler::new_themed(GraphicalTheme::none()).with_width(80)
.with_links(false)` (`lib.rs:72-79`) — deterministic, no ANSI, which is what we want inside an error
`Display`. Reaching for `GraphicalReportHandler` directly is only needed for colour or for
`render_reports` (shared line-index across many diagnostics, `handlers/graphical/report.rs:60`);
not worth it on a cold path.

**Error type shape.** `OxcDiagnostic` carries only spans — it is useless without the source text, and
storing both source + diagnostics in the error just to render later is more state than den needs
(all three consumers immediately do `e.to_string()`; `http.rs:90`, `mmap_script.rs:67`). So render
eagerly at the boundary and carry a `String`:

```rust
#[derive(Debug, Display, Error)]
pub enum EasyOxcTranspilerError {
    #[display("failed to parse source:\n{_0}")]
    Parse(#[error(not(source))] String),
    #[display("failed to analyse source:\n{_0}")]
    Semantic(#[error(not(source))] String),
    #[display("failed to transform source:\n{_0}")]
    Transform(#[error(not(source))] String),
}
```

Gone vs. the swc version: `SwcParse(anyhow::Error)` (so **the `anyhow` dep goes away**),
`SwcEmitProgram(io::Error)` (codegen is infallible and returns `String`), `Utf8(FromUtf8Error)`
(no `Vec<u8>` step). `den-core` only ever `#[from]`-converts this type (`engine.rs:382-384`), never
matches on variants, so the shape change is free. Sample output:

```
failed to parse source:

  x Unexpected token
   ,-[<anonymous>:1:7]
 1 | const = ;
   :       ^
   `----
```

Also check `ParserReturn::panicked` (`oxc_parser/src/lib.rs:188`) — when true the AST is empty and
feeding it to codegen would silently produce an empty module instead of an error.

---

## 6. The allocator lifetime problem

### There isn't one, if you scope the arena inside `transpile`

`Allocator` is `#[derive(Default)] #[repr(transparent)] pub struct Allocator { arena: Arena }`
(`oxc_allocator-0.146.0/src/allocator.rs:216-220`). Everything downstream (`Program<'a>`,
`Semantic<'a>`, `Codegen<'a>`, `CodegenReturn<'a>`) is parameterised by the borrow of that
`Allocator`, and `Parser::new(&'a Allocator, &'a str, _)` additionally unifies `'a` with the source
text borrow.

The rule: **create the `Allocator` as a local in `transpile`, and make sure the two values you return
are owned.** They are:

- `CodegenReturn.code: String` — already owned.
- `CodegenReturn.map: Option<SourceMap<'a>>` — borrowed; call `.into_owned()` (§3). This is the one
  place the compiler will bite you.

Nothing else escapes. `Scoping` has no lifetime, and `Diagnostics` is rendered to `String` before
return. The `Allocator` drops at the end of the function and frees the whole arena in one go — which
is the arena's whole point (`oxc/ARCHITECTURE.md:36`, "single allocation arena for entire
compilation unit").

### The struct becomes stateless — yes, really

swc needed `Lrc<SourceMap>` (file interning), `SwcComments`, `Handler`, and `Globals` +
`GLOBALS.set(...)` to make `Mark::new()` work. **oxc needs none of it**: comments live on
`Program.comments`, spans are plain `u32` offsets, symbol identity lives in `Scoping`, and
diagnostics are returned by value. So:

```rust
#[derive(Default)]
pub struct EasyOxcTranspiler;
```

A ZST. `Send + Sync + 'static` for free (asserted in the test suite), so `Arc<EasyOxcTranspiler>`
shared across tokio tasks needs no thought, and 8 concurrent threads transpiling through one `Arc`
was verified working. Keep `#[derive(Default)]` — `Arc<T>: Default` needs it for the loaders'
`derivative(Default)`.

`&self` is retained on `transpile` purely so `self.transpiler.transpile(...)` at the three call
sites keeps compiling.

### `AllocatorPool` — skip it for now

It exists: `oxc_allocator::AllocatorPool::new(thread_count) -> AllocatorPool`, `.get() ->
AllocatorGuard<'_>` (derefs to `Allocator`, resets and returns it on drop) —
`oxc_allocator-0.146.0/src/pool/mod.rs:20-80`. It is behind the non-default `pool` cargo feature
(`Cargo.toml [features] pool = []`).

Not worth it here. den transpiles a handful of module files at load time plus REPL lines; the win is
one `malloc` of the first arena chunk per call. If profiling ever says otherwise the upgrade is
three lines:

```toml
oxc_allocator = { version = "0.146.0", features = ["pool"] }
```
```rust
pub struct EasyOxcTranspiler { arenas: AllocatorPool }   // Default: AllocatorPool::new(num_cpus)
let allocator = self.arenas.get();                       // then &*allocator everywhere
```

Mark it: `// ponytail: fresh arena per call; AllocatorPool if transpile shows up in a profile`.

---

## 7. Feature gating

`oxc_transformer` is one crate doing both TS and JSX, so `typescript` and `react` both have to pull
it in. Same for `oxc_semantic` — den names it *only* to feed `Scoping` to the transformer.

> ⚠️ Making `oxc_semantic` optional does **not** shrink the build. `oxc_codegen 0.146` declares
> `oxc_semantic = "0.146.0"` as a **mandatory** (non-optional) dependency
> (`oxc_codegen-0.146.0/Cargo.toml [dependencies.oxc_semantic]`), and likewise `oxc_ast`. So
> `oxc_semantic` and `oxc_ast` compile in *every* configuration, including transpile-only. The
> `optional = true` below buys exactly one thing: the `use oxc_semantic::SemanticBuilder` inside the
> `cfg(any(typescript, react))` block doesn't need a separate `cfg`, and `cargo` won't warn about an
> unused direct dependency. Do it for tidiness, not for compile time.

| feature | pulls | effect in code |
|---|---|---|
| `transpile` | `oxc_allocator`, `oxc_parser`, `oxc_codegen`, `oxc_span`, `oxc_sourcemap`, `oxc_diagnostics` | `#![cfg(feature = "transpile")]`; parse → codegen round-trip only |
| `typescript` | `+ dep:oxc_transformer, dep:oxc_semantic` | `.ts/.mts/.cts` map to a TS `SourceType` in `infer_…`; `is_typescript()` then drives the strip at `oxc_transformer/src/lib.rs:161` |
| `react` | `+ dep:oxc_transformer, dep:oxc_semantic` | `.jsx/.mjsx` map to a JSX `SourceType`; `JsxOptions::enable()` w/ `Classic` instead of `disable()` |
| `typescript` + `react` | — | `.tsx` enabled |

Use `#[cfg(any(feature = "typescript", feature = "react"))]` for the semantic+transform block (two
sites). An internal `_transformer` feature would read slightly better but is one more knob for no
gain.

`oxc_codegen`'s `sourcemap` feature is **on by default** (`oxc_codegen-0.146.0/Cargo.toml [features]
default = ["sourcemap"]`) — leave default features on or `CodegenReturn.map` won't exist.

---

## 8. Cargo.toml (verified: `cargo tree` resolves, single `oxc_sourcemap`)

```toml
[dependencies]
derive_more = { workspace = true, features = ["display", "debug", "error"] }

oxc_allocator   = "0.146.0"
oxc_codegen     = "0.146.0"
oxc_diagnostics = "0.146.0"
oxc_parser      = "0.146.0"
oxc_semantic    = { version = "0.146.0", optional = true }
oxc_sourcemap   = "8.1.2"
oxc_span        = "0.146.0"
oxc_transformer = { version = "0.146.0", optional = true }

[features]
default = ["transpile"]

typescript = ["transpile", "dep:oxc_transformer", "dep:oxc_semantic"]
react      = ["transpile", "dep:oxc_transformer", "dep:oxc_semantic"]
transpile  = []
```

Deltas from the current `den-transpiler-oxc/Cargo.toml`:

- **drop `oxc_ast`** — never named; `Program` only appears via inference. (It still gets built —
  `oxc_codegen` depends on it unconditionally. This is a tidiness change, not a build-time one.)
- **drop `trie-match`** — an 8-arm `match` on `&str`; the trie proc-macro earns nothing here.
- **add `oxc_semantic` as `optional = true`** — see the warning in §7: cosmetic, not a build saving.
- **add `dep:oxc_semantic`** to `typescript` and `react`.
- `derive_more` is a **workspace** dependency whose feature list already includes
  `from, into, deref, deref_mut, display, error, debug` (root `Cargo.toml`). Per-crate `features =
  [...]` is *additive*, so the three listed above are a no-op subset and `#[from]` would work even
  though the §5 enum doesn't use it. Nothing to change here; the current file is already right.
- **no change needed in `den-core/Cargo.toml`.** It already depends on `den-transpiler-oxc` with
  `default-features = false` and wires `typescript`/`react`/`transpile` through
  `den-transpiler-oxc?/…` (lines 44, 53-61). Because `default-features = false`, the transpiler
  crate's own `default = ["transpile"]` never fires from den-core — `transpile` always arrives
  explicitly. Verified: `cargo check -p den-core --no-default-features --features X` for
  `X ∈ {transpile, typescript, react, typescript,react}` produces zero transpiler-related
  diagnostics.

`cargo tree -i oxc_sourcemap` → one copy at 8.1.2, shared with `oxc_codegen` (which declares
`^8.0.2`). No duplicate.

Also delete the now-unused `sourcemap`, `anyhow`, and every `swc_*` entry from the workspace if
nothing else uses them.

---

## 9. The whole new `den-transpiler-oxc/src/lib.rs`

Compiles and passes tests as written. Drop it in verbatim.

```rust
#![cfg(feature = "transpile")]

use std::path::Path;

use derive_more::{Debug, Display, Error};
use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions, CodegenReturn};
use oxc_diagnostics::{NamedSource, OxcDiagnostic};
use oxc_parser::{Parser, ParserReturn};
pub use oxc_sourcemap::OwnedSourceMap as SourceMap;
pub use oxc_span::SourceType as Syntax;

/// Virtual file name used for diagnostics, sourcemap `sources[0]`, and the
/// transformer's `source_path`. den transpiles in-memory buffers whose real
/// path is not always known at this layer.
const ANONYMOUS_SOURCE: &str = "<anonymous>";

/// swc's `IsModule`, kept so den's call sites stay untouched.
#[derive(Debug, Display, Clone, Copy, PartialEq, Eq)]
pub enum IsModule {
    /// Force module (`true`) or script (`false`) parsing.
    Bool(bool),
    /// Decide from the source: ESM syntax anywhere means module.
    Unknown,
}

impl IsModule {
    fn apply(self, syntax: Syntax) -> Syntax {
        match self {
            Self::Bool(true) => syntax.with_module(true),
            Self::Bool(false) => syntax.with_script(true),
            Self::Unknown => syntax.with_unambiguous(true),
        }
    }
}

/// Stateless: oxc keeps no interner, no comment store and no thread-local
/// globals, so there is nothing to hold between calls. `Default` is kept
/// because the loaders' `derivative(Default)` needs `Arc<Self>: Default`.
#[derive(Default)]
pub struct EasyOxcTranspiler;

impl EasyOxcTranspiler {
    pub fn transpile(
        &self,
        source: &str,
        syntax: Syntax,
        is_module: IsModule,
        emit_sourcemap: bool,
    ) -> Result<(String, Option<SourceMap>), EasyOxcTranspilerError> {
        // The arena and everything borrowed from it live and die inside this call.
        // ponytail: fresh arena per call; AllocatorPool if transpile shows up in a profile.
        let allocator = Allocator::new();

        #[allow(unused_mut)]
        let ParserReturn { mut program, diagnostics, panicked, .. } =
            Parser::new(&allocator, source, is_module.apply(syntax)).parse();
        if panicked || diagnostics.has_errors() {
            return Err(EasyOxcTranspilerError::Parse(EasyOxcTranspilerError::render(
                source,
                diagnostics,
            )));
        }

        #[cfg(any(feature = "typescript", feature = "react"))]
        {
            use oxc_semantic::SemanticBuilder;
            use oxc_transformer::Transformer;

            // `with_enum_eval(true)` is not optional: the TS enum transform asserts on it.
            // `into_scoping()` drops the shared borrow so the transformer can take `&mut program`.
            let semantic = SemanticBuilder::new_compiler()
                .with_enum_eval(true)
                .with_excess_capacity(2.0)
                .build(&program);
            if semantic.diagnostics.has_errors() {
                return Err(EasyOxcTranspilerError::Semantic(EasyOxcTranspilerError::render(
                    source,
                    semantic.diagnostics,
                )));
            }
            let scoping = semantic.semantic.into_scoping();

            let transformed =
                Transformer::new(&allocator, Path::new(ANONYMOUS_SOURCE), &Self::transform_options())
                    .build_with_scoping(scoping, &mut program);
            if transformed.diagnostics.has_errors() {
                return Err(EasyOxcTranspilerError::Transform(EasyOxcTranspilerError::render(
                    source,
                    transformed.diagnostics,
                )));
            }
        }

        let CodegenReturn { code, map, .. } = Codegen::new()
            .with_options(CodegenOptions {
                source_map_path: emit_sourcemap.then(|| Path::new(ANONYMOUS_SOURCE).to_path_buf()),
                ..CodegenOptions::default()
            })
            .build(&program);

        // The map borrows the arena and the source text, so detach it before the arena drops.
        Ok((code, map.map(|map| SourceMap::new(map.into_owned()))))
    }

    /// Strip types, keep native class fields, downlevel nothing, and use the
    /// classic JSX runtime so output stays resolvable without a `react` module.
    #[cfg(any(feature = "typescript", feature = "react"))]
    fn transform_options() -> oxc_transformer::TransformOptions {
        oxc_transformer::TransformOptions {
            jsx: if cfg!(feature = "react") {
                oxc_transformer::JsxOptions {
                    runtime: oxc_transformer::JsxRuntime::Classic,
                    ..oxc_transformer::JsxOptions::enable()
                }
            } else {
                oxc_transformer::JsxOptions::disable()
            },
            ..oxc_transformer::TransformOptions::default()
        }
    }
}

#[derive(Debug, Display, Error)]
pub enum EasyOxcTranspilerError {
    #[display("failed to parse source:\n{_0}")]
    Parse(#[error(not(source))] String),
    #[display("failed to analyse source:\n{_0}")]
    Semantic(#[error(not(source))] String),
    #[display("failed to transform source:\n{_0}")]
    Transform(#[error(not(source))] String),
}

impl EasyOxcTranspilerError {
    /// oxc diagnostics are span-only, so they must be rendered while the source
    /// text is still in hand.
    fn render(source: &str, diagnostics: impl IntoIterator<Item = OxcDiagnostic>) -> String {
        diagnostics
            .into_iter()
            .map(|diagnostic| {
                diagnostic.render_with_source_code(NamedSource::new(ANONYMOUS_SOURCE, source))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn infer_transpile_syntax_by_extension(extension: &str) -> Option<Syntax> {
    match extension {
        "js" | "mjs" | "cjs" => Syntax::from_extension(extension).ok(),
        // oxc has no `mjsx`, and `jsx` would come back Unambiguous; both are always
        // overridden by `IsModule` at transpile time anyway.
        "jsx" | "mjsx" => cfg!(feature = "react").then(Syntax::jsx),
        "ts" | "mts" | "cts" => cfg!(feature = "typescript")
            .then(|| Syntax::from_extension(extension).ok())
            .flatten(),
        "tsx" => cfg!(all(feature = "typescript", feature = "react")).then(Syntax::tsx),
        _ => None,
    }
}

#[derive(Display, Error, Debug)]
pub enum InferTranspileSyntaxError {
    InvalidExtension,
}

pub const fn get_best_transpiling() -> &'static str {
    match (cfg!(feature = "typescript"), cfg!(feature = "react")) {
        (false, false) => "js",
        (false, true) => "jsx",
        (true, false) => "ts",
        (true, true) => "tsx",
    }
}
```

---

## 10. `den-core` changes (mechanical)

Three files: `den-core/src/engine.rs`, `den-core/src/loader/http.rs`,
`den-core/src/loader/mmap_script.rs`.

```sh
cd /home/steve/git/github.com/stevefan1999-personal/den/den-core/src
sed -i 's/den_transpiler_swc/den_transpiler_oxc/g; s/EasySwcTranspiler/EasyOxcTranspiler/g' \
  engine.rs loader/http.rs loader/mmap_script.rs
```

`EasySwcTranspiler` → `EasyOxcTranspiler` also fixes `EasySwcTranspilerError` (prefix match) and the
`EngineError::EasySwcTranspiler(..)` variant name at `engine.rs:384`. Nothing else changes: the
`transpile` signature, `IsModule::Bool(true)`, `IsModule::Unknown`, `.unwrap_or_default()`, and
`Option<SourceMap>` in `Engine::transpile` are all preserved.

The mmap loader's registered extension list (`engine.rs:201-217`: `js`, `mjs`, +`jsx`/`mjsx` under
`react`, +`ts`, +`tsx`) is unchanged and every entry is handled by
`infer_transpile_syntax_by_extension`.

Housekeeping: `HEAD` still contains `den-transpiler-swc/src/transpile.rs`, a dead copy of
`infer_transpile_syntax_by_extension` / `InferTranspileSyntaxError` / `get_best_transpiling` that was
never `mod`-declared from `lib.rs`. The working tree has already deleted it. Do not resurrect it.

### 10.1 🔴 den-core does not compile even after this change — and none of it is the transpiler

This was measured: a clean copy of the working tree, §9's `lib.rs` and §8's `Cargo.toml` dropped in,
§10's `sed` applied, then `cargo check -p den-core --no-default-features --features typescript,react`.
The transpiler contributes **zero** errors and **zero** warnings. What is left is unrelated
dependency drift that an implementer will otherwise blame on this migration:

| error | site | cause |
|---|---|---|
| `E0050: method load has 3 parameters but the trait has 4` | `den-core/src/loader/http.rs:25`, `den-core/src/loader/mmap_script.rs:40` | rquickjs 0.12 `Loader::load` gained `attributes: Option<ImportAttributes<'js>>` |
| `E0050: method resolve has 4 parameters but the trait has 5` | `den-core/src/resolver/http.rs:12` | rquickjs 0.12 `Resolver::resolve` gained the same parameter |
| `E0425: cannot find function block_in_place in tokio::task` | `http.rs:107`, `mmap_script.rs:79` | den-core's `tokio.workspace = true` doesn't enable `rt-multi-thread` |
| `E0133: call to unsafe function AsyncMmapFile::open` | `mmap_script.rs:53` | fmmap 0.5 made `open` `unsafe` |
| `warning: use of deprecated macro async_with` | `engine.rs:5,313,359` | rquickjs 0.12 deprecation |

Do these in a separate commit. Once they are patched, den-core builds clean under all four
transpiler feature combinations (`transpile`, `+typescript`, `+react`, `+typescript,react`).

---

## 11. Behaviour deltas to expect (measured, not guessed)

| | swc (before) | oxc (after) |
|---|---|---|
| Indentation | 4 spaces | **tab, width 1** (`IndentChar::default() == Tab`, `DEFAULT_INDENT_WIDTH == 1`). Set `indent_char: IndentChar::Space, indent_width: 2` in `CodegenOptions` if it bothers you. |
| Trailing semicolons | `omit_last_semi = true` | always emitted |
| JSX pure annotations | swc emits them too | `/* @__PURE__ */` (`JsxOptions.pure` default `true`) |
| Codegen `target` | `EsVersion::Es2022` on the emitter | no target concept; oxc prints what's in the AST. Since neither pipeline actually downlevelled, output parity holds. |
| Identifier renaming | `hygiene()` renamed shadowed bindings | oxc renames only when a transform must (e.g. namespace `_N`). Cleaner output. |
| Empty statements | `fixer()` cleaned some | `let a=1;;` → `let a = 1;\n;\n` (kept) |
| Diagnostics | printed to stderr by `Handler::with_tty_emitter`, error was `anyhow` | never printed; rendered into the error string. **Warnings no longer reach stderr at all** — `has_errors()` filters by `Severity` (`oxc_diagnostics/src/lib.rs:96-99`). |
| **Decorators** | **parse error** (`TsSyntax::decorators` defaults `false`) | parsed and **emitted verbatim** → QuickJS `SyntaxError` at `Module::declare`. See §2. |
| **`const enum`** | swc inlined members | `optimize_const_enums` defaults `false` → runtime IIFE is kept: `var E = function(E){ E[E["A"]=1]="A"; return E; }(E \|\| {})` |
| Comments / shebang | preserved | preserved — `CommentOptions::default()` is `normal/jsdoc/annotation = true`, `legal = Inline` (`oxc_codegen/src/options.rs`). A trailing same-line `/* block */` is dropped ("at present only statement level comments are printed"). |
| `export =` / `import x = require()` | rewritten to CJS | same (`module.exports = 1;` / `const x = require("y");`) — the TS transform does this regardless of `Module::Preserve` |
| `abstract` / overload signatures / `<T>x` assertions / `declare module` | stripped | stripped identically |

Verified outputs:

```
in : const x: number = 1; interface Foo { a: string } export {};
out: const x = 1;\nexport {};

in : const a = <div x={1 as number}>hi</div>;                       (tsx, Unknown)
out: const a = /* @__PURE__ */ React.createElement("div", { x: 1 }, "hi");

in : enum E { A, B } namespace N { export const q = 1 }             (ts, Bool(false))
out: …IIFE-wrapped enum + namespace, requires with_enum_eval(true)

in : export const a = 1                                             (js, Unknown)
out: export const a = 1;          ← resolved to Module

in : var a = 1                                                      (js, Unknown)
out: var a = 1;                   ← resolved to Script
```

---

## 12. Tests to land with the change

Drop this in as `den-transpiler-oxc/tests/smoke.rs` (inlined here on purpose — the scratch crate that
validated this doc lives under `/tmp` and will not survive). Run it with
`cargo nextest run -p den-transpiler-oxc --features typescript,react`.

It covers, one behaviour each:

1. TS annotations + `interface` stripped
2. JSX → `React.createElement` (guards against the Automatic-runtime default)
3. TSX → `React.createElement`
4. `IsModule::Unknown` resolving both ways
5. top-level `await import(...)` under `Unknown` (den's REPL path)
6. sourcemap emitted and detached from the arena (`to_json_string()` after the arena drops)
7. `emit_sourcemap = false` really yields `None` (den's only real call pattern)
8. class fields stay native (the `native_class_properties` equivalence)
9. **`enum` + `namespace`** — the `with_enum_eval` regression guard; it *panics* without it
10. parse error renders with a source snippet
11. the extension → `SourceType` table, asserted against the active features
12. `EasyOxcTranspiler: Send + Sync + 'static` and 8 threads through one `Arc`

Nothing needs a fixture or a framework. Every test is either feature-agnostic or `cfg`-gated, so the
file is green under all four combinations — measured: `transpile` 7 pass, `+typescript` 10,
`+react` 8, `+typescript,react` 12, zero failures.

```rust
use den_transpiler_oxc::*;

fn run(src: &str, ext: &str, m: IsModule) -> (String, Option<SourceMap>) {
    let syntax = infer_transpile_syntax_by_extension(ext).unwrap_or_default();
    EasyOxcTranspiler.transpile(src, syntax, m, true).unwrap()
}

#[cfg(feature = "typescript")]
#[test]
fn ts_types_are_stripped() {
    let (code, _) = run(
        "const x: number = 1; interface Foo { a: string } export {};",
        "ts",
        IsModule::Bool(true),
    );
    assert!(!code.contains("interface"));
    assert!(!code.contains(": number"));
}

/// Guards against `JsxOptions::default()`, whose Automatic runtime would emit
/// `import { jsx as _jsx } from "react/jsx-runtime"` — unresolvable in den.
#[cfg(feature = "react")]
#[test]
fn jsx_becomes_create_element() {
    let (code, _) = run("const a = <div x={1}>hi</div>;", "jsx", IsModule::Bool(true));
    assert!(code.contains("React.createElement"));
    assert!(!code.contains("jsx-runtime"));
}

#[cfg(all(feature = "typescript", feature = "react"))]
#[test]
fn tsx_becomes_create_element() {
    let (code, _) = run("const a = <div x={1 as number}>hi</div>;", "tsx", IsModule::Unknown);
    assert!(code.contains("React.createElement"));
}

#[test]
fn unknown_resolves_script_vs_module() {
    let (script, _) = run("var a = 1", "js", IsModule::Unknown);
    assert!(!script.contains("export"));
    let (module, _) = run("export const a = 1", "js", IsModule::Unknown);
    assert!(module.contains("export"));
}

/// den's REPL path: `Engine::eval` uses `get_best_transpiling()` + `IsModule::Unknown`, so
/// top-level `await` must upgrade the source to ESM rather than fail.
#[test]
fn top_level_await_import_in_unknown() {
    let (code, _) = run("await import(`./x.js`)", get_best_transpiling(), IsModule::Unknown);
    assert!(code.contains("import("));
}

/// The map borrows the arena and the source text; this trips if `into_owned` is dropped.
#[test]
fn sourcemap_is_emitted_and_owned() {
    let (_, map) = run("const x = 1; export {};", "js", IsModule::Bool(true));
    let json = map.expect("sourcemap").to_json_string();
    assert!(json.contains("\"mappings\""));
    assert!(json.contains("<anonymous>"));
}

#[test]
fn no_sourcemap_when_not_requested() {
    let syntax = infer_transpile_syntax_by_extension("js").unwrap();
    let (_, map) = EasyOxcTranspiler
        .transpile("const x = 1;", syntax, IsModule::Bool(true), false)
        .unwrap();
    assert!(map.is_none());
}

/// Equivalent of swc's `native_class_properties = true`.
#[cfg(feature = "typescript")]
#[test]
fn class_fields_stay_native() {
    let (code, _) = run(
        "class A { x = 1; declare y: number; z!: string; }",
        "ts",
        IsModule::Bool(true),
    );
    assert!(code.contains("x = 1"));
    assert!(!code.contains("declare"));
}

/// Regression guard for `SemanticBuilder::with_enum_eval(true)` — this *panics* without it.
#[cfg(feature = "typescript")]
#[test]
fn enum_and_namespace() {
    let (code, _) = run(
        "enum E { A, B } namespace N { export const q = 1 }",
        "ts",
        IsModule::Bool(false),
    );
    assert!(code.contains("E[E[\"A\"]"));
}

#[test]
fn parse_error_is_rendered() {
    let syntax = infer_transpile_syntax_by_extension("js").unwrap();
    let err = EasyOxcTranspiler
        .transpile("const = ;", syntax, IsModule::Unknown, false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("failed to parse source"));
    assert!(err.contains("<anonymous>"), "diagnostic must carry the source snippet");
}

#[test]
fn extension_table() {
    for ext in ["js", "mjs", "cjs"] {
        assert!(infer_transpile_syntax_by_extension(ext).is_some(), "{ext}");
    }
    for ext in ["json", "", "d.ts", "wasm"] {
        assert!(infer_transpile_syntax_by_extension(ext).is_none(), "{ext}");
    }
    for ext in ["ts", "mts", "cts"] {
        assert_eq!(
            infer_transpile_syntax_by_extension(ext).is_some(),
            cfg!(feature = "typescript"),
            "{ext}"
        );
    }
    for ext in ["jsx", "mjsx"] {
        assert_eq!(
            infer_transpile_syntax_by_extension(ext).is_some(),
            cfg!(feature = "react"),
            "{ext}"
        );
    }
    assert_eq!(
        infer_transpile_syntax_by_extension("tsx").is_some(),
        cfg!(all(feature = "typescript", feature = "react"))
    );
    // Whatever `get_best_transpiling` picks must be inferable in this build.
    assert!(infer_transpile_syntax_by_extension(get_best_transpiling()).is_some());
}

/// `Arc<EasyOxcTranspiler>` is a hard requirement of the loaders' `derivative(Default)`.
#[test]
fn transpiler_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<EasyOxcTranspiler>();
    assert_send_sync::<SourceMap>();

    let shared = std::sync::Arc::new(EasyOxcTranspiler::default());
    let handles: Vec<_> = (0..8)
        .map(|n| {
            let shared = shared.clone();
            std::thread::spawn(move || {
                let syntax =
                    infer_transpile_syntax_by_extension(get_best_transpiling()).unwrap();
                let src = format!("export const w{n} = {n};");
                shared.transpile(&src, syntax, IsModule::Bool(true), true).unwrap().0
            })
        })
        .collect();
    for (n, handle) in handles.into_iter().enumerate() {
        assert!(handle.join().unwrap().contains(&format!("w{n}")));
    }
}
```

---

## Verification log

Second pass, 2026-08-22, by a completeness critic working independently of the original author.
Everything below was checked against the local crate sources under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` and the den working tree (not HEAD — the
swc→oxc rename is staged but uncommitted).

### Executable verification performed

| what | how | result |
|---|---|---|
| §9 `lib.rs` compiles as written | extracted the §9 code block verbatim into a fresh crate with §8's `Cargo.toml`, built with `[]`, `[typescript]`, `[react]`, `[typescript,react]` | ✅ 4/4, zero warnings |
| §12 suite passes | extracted the §12 code block verbatim, ran `cargo test` in all 4 combos | ✅ 7 / 10 / 8 / 12 passed, 0 failed |
| §10 sed produces a working den-core | rsync'd the working tree to a scratch dir, dropped in §8 + §9, ran the §10 `sed`, then `cargo check -p den-core --no-default-features --features X` for all 4 combos | ✅ zero transpiler-related errors or warnings in all 4 |
| §2 JSX/TS output claims | ran a probe binary over spread attrs, fragments, child spread, namespaced attrs, parameter properties, `const enum`, `import type`, `satisfies`, `accessor`, decorators, `export =`, `import x = require()`, `declare module`, `abstract`, overloads, `<T>x`, comments, shebang | see §2 and §11 |

### Claims re-read in the source and confirmed correct

- `Parser::new(&'a Allocator, &'a str, SourceType)` in `impl<'a> Parser<'a>`; `parse(self)` in
  `impl<'a, C: ParserConfig> Parser<'a, C>` — `oxc_parser-0.146.0/src/lib.rs:281,361`.
- All seven `ParserReturn` fields and `#[non_exhaustive]` — `oxc_parser/src/lib.rs:149-193`.
- `SemanticBuilder::{new_compiler,with_enum_eval,with_excess_capacity,build}` at
  `oxc_semantic-0.146.0/src/builder.rs:191,248,303,311`; `SemanticBuilderReturn` at `:131-135`;
  `Semantic::into_scoping(self) -> Scoping` (no lifetime) at `src/lib.rs:112`. **Exactly as claimed.**
- `Transformer::new(&'a Allocator, &Path, &TransformOptions)` `:117`, `build_with_scoping` `:132`,
  `TransformerReturn` `#[non_exhaustive]` with a `#[deprecated] helpers_used` `:87-96`.
- `CodegenReturn { code: String, map: Option<oxc_sourcemap::SourceMap<'a>>, legal_comments }`,
  `map` behind `#[cfg(feature = "sourcemap")]`, and `default = ["sourcemap"]` in
  `oxc_codegen-0.146.0/Cargo.toml:51`. **Confirmed — leaving default features off would delete the
  field, not just the data.**
- `CodegenOptions::source_map_path: Option<PathBuf>` is the on/off switch; `IndentChar::default() ==
  Tab` and `DEFAULT_INDENT_WIDTH == 1` (`oxc_data_structures-0.146.0/src/code_buffer.rs:12,19`).
- `oxc_sourcemap-8.1.2`: `SourceMap::into_owned() -> SourceMap<'static>` `:102`,
  `OwnedSourceMap` `:36` with `new(SourceMap<'static>)` `:43`, `Debug + Clone + Default`.
- `SourceType`: `pub(super)` fields `:24-36`, `Default == mjs()` `:96-100`, `VALID_EXTENSIONS` `:119`,
  `from_extension` `:639`, all builders `const fn(self, bool)`. The whole extension table in §4 was
  re-measured and matches.
- `oxc_diagnostics::Diagnostics` is a `Vec<OxcDiagnostic>` newtype with `has_errors`/`has_warnings`/
  `errors`/`warnings`/`into_vec`/`Deref`/`IntoIterator` (`src/lib.rs:92-180`);
  `OxcDiagnostic::render` `:308`, `render_with_source_code(self, T)` `:487`; `NamedSource` and
  `SourceCode` re-exported at `:61-65`; `str`/`&str`/`String`/`Arc<T>: SourceCode`
  (`source_impls.rs:767-786`).
- `TypeScriptOptions` field-by-field, including `allow_declare_fields: true` and
  `remove_class_fields_without_initializer: false` feeding
  `x2_es2022: ES2022::new(env.es2022, !allow_declare_fields || remove_class_fields_without_initializer)`
  at `oxc_transformer/src/lib.rs:174-177`. The "native class fields" reasoning holds.
- `JsxOptions::default() == enable()` with `JsxRuntime::Automatic` as `#[default]`. The 🔴 warning is
  correct and load-bearing.
- The helper-loader grep (`helper_call|helper_load|Helper::` in `src/typescript/`, `src/jsx/`)
  really does return nothing.

### Claims corrected

1. **§7 / §8 — `oxc_semantic` optionality.** The doc said making it `optional = true` stops it being
   "forced into transpile-only builds". Wrong: `oxc_codegen 0.146` declares
   `[dependencies.oxc_semantic] version = "0.146.0"` **non-optional** (and likewise `oxc_ast`), so
   both compile in every configuration. Rewritten as a tidiness change with an explicit ⚠️ note; the
   "drop `oxc_ast`" bullet got the same caveat.
2. **§8 — `derive_more` features.** The doc implied you might need to add `from`. den's *workspace*
   `derive_more` already enables `from, into, deref, deref_mut, display, error, debug`, and per-crate
   feature lists are additive, so the current file is already correct. Noted.
3. **Line-number drift**, corrected throughout: `oxc_parser` `parse` 350→361, `ParserReturn` range;
   `TransformerReturn` 86-95→87-96; `CodegenReturn` 52-66→51-68; `options/mod.rs` 38-84→39-85 and
   `enable_all` 90→92; transformer `lib.rs` 161-164→163-166, 167-170→174-177, 145-156→147-157;
   `jsx/options.rs` 36→37, `Default` 117→118, added `enable` 135 / `disable` 155, runtime 17-19→14-19;
   `helper_loader.rs` 104-137→120-137; `source_type.rs` VALID_EXTENSIONS 116→119, `from_extension`
   638→639, default 95-99→96-100, per-constructor and per-builder lines; `oxc_diagnostics/src/lib.rs`
   89-127→92-130, render 293→308, `render_with_source_code` 470→487, graphical handler 70-77→72-79;
   `owned_sourcemap.rs` 5-13,43→36,43; `oxc_parser` `panicked` 183→188, TLA comment 1727→1722;
   den-side `engine.rs:13-16`→`12-18`, `382-389`→`381-390`, `200-217`→`201-217`.

### Gaps filled

4. **§2 — decorators (new subsection).** The doc never mentioned them, and this is a real regression:
   swc's `TsSyntax::decorators` defaults to `false` (`swc_ecma_parser-6.0.0/src/lib.rs:316`), so
   `@dec class A {}` was a **parse error** under den-with-swc. oxc parses it and emits it **verbatim**
   (measured), so den now fails downstream inside QuickJS with a bare `SyntaxError`. Turning on
   `DecoratorOptions { legacy: true }` does transform it, but injects
   `import _decorate from "@oxc-project/runtime/helpers/decorate"` — also unresolvable in den
   (measured). Three options written up; §9 keeps the default, now as a documented choice rather than
   an oversight. This is also the one thing that wakes the otherwise-dormant helper loader, so the
   "Helpers: nothing to configure" heading was qualified.
5. **§10.1 (new) — den-core still does not compile, and none of it is the transpiler.** Measured on a
   clean tree with the doc applied verbatim: rquickjs 0.12 added `Option<ImportAttributes<'js>>` to
   `Loader::load` (`http.rs:25`, `mmap_script.rs:40`) and `Resolver::resolve` (`resolver/http.rs:12`);
   `tokio::task::block_in_place` needs `rt-multi-thread`, which den-core's `tokio.workspace = true`
   omits (`http.rs:107`, `mmap_script.rs:79`); fmmap 0.5 made `AsyncMmapFile::open` `unsafe`
   (`mmap_script.rs:53`); `rquickjs::async_with` is deprecated (`engine.rs:5,313,359`). Without this
   table an implementer will follow the doc, see five errors, and conclude the doc is wrong.
6. **§8 — den-core's `Cargo.toml` needs no change.** Previously unstated. Added, with the reason
   (`default-features = false` means the transpiler's own `default = ["transpile"]` never fires) and
   the four-combo `cargo check` evidence.
7. **§4 — `cjs`/`mts`/`cts` are newly accepted but unreachable** (`engine.rs:201-217` registers only
   six extensions; `http.rs` derives only `"js"`/`"ts"` from MIME). Added so nobody hunts for a
   behaviour change that isn't there.
8. **§11 — five missing behaviour-delta rows**: decorators, `const enum` (not inlined —
   `optimize_const_enums` defaults `false`), comment/shebang handling, `export =` /
   `import x = require()`, and the stripped-identically set (`abstract`, overload signatures,
   `<T>x` assertions, `declare module`).
9. **§2 — measured classic-runtime JSX output** for spread attributes, fragments, child spread and
   namespaced attributes, since those are the cases where a JSX transform normally reaches for a
   helper. None do.
10. **§12 — the test suite is now inlined.** It previously pointed at a path under
    `/tmp/claude-1000/…/scratchpad/proto/tests/smoke.rs`, which is session-scoped and already gone by
    the time anyone reads this. The inlined version is also **fixed**: the scratch original had no
    `cfg` gates and failed 8/10 under `--features transpile` and 8/10 under `--features react`
    (`infer_transpile_syntax_by_extension("ts"/"tsx")` returns `None`, `unwrap_or_default()` falls back
    to `mjs`, and the TS/JSX inputs then fail to parse). Tests are now feature-gated, two were
    rewritten to feature-agnostic inputs, and two were added (`jsx_becomes_create_element` so the
    `react`-only build is actually exercised; `no_sourcemap_when_not_requested`, which is den's only
    real call pattern). Green in all four combos.
11. **§10 — `den-transpiler-swc/src/transpile.rs`** exists in `HEAD` as a dead, never-`mod`-declared
    duplicate of the three free functions. Already deleted in the working tree; noted so it isn't
    restored during the rename.
12. **§1 — `CodegenReturn` is `#[non_exhaustive]`**, which the doc flagged for `ParserReturn` and
    `TransformerReturn` but not here. §9's `let CodegenReturn { code, map, .. }` is legal only because
    of the trailing `..`.
13. **§5 — `Diagnostics: IntoIterator<Item = OxcDiagnostic>`** (`lib.rs:169`) added, since that is the
    non-obvious reason §9's `render(source, diagnostics)` can take `impl IntoIterator` and be handed a
    `Diagnostics` by value.

### Not changed (checked, already right)

- The `with_enum_eval(true)` panic (§1.2) — reproduced; the suite's `enum_and_namespace` test is the
  guard and it is real.
- The `OwnedSourceMap` decision (§3) and the "den never reads the sourcemap" grep.
- The stateless-ZST argument and the `AllocatorPool` deferral (§6). `pool` is indeed a non-default
  feature.
- The `IsModule` → `with_module`/`with_script`/`with_unambiguous` mapping (§4) and the parser's
  unambiguous resolution + top-level-`await` ESM upgrade.
- Every den-side line anchor in §0 and §10 (`engine.rs:335,340,341,350,384,389`, `http.rs:83,85,90`,
  `mmap_script.rs:61,63,67`) — all exact.
