/**
 * `den:ffi` — the published type surface.
 *
 * The schema is plain data: one value that types the call site here, builds
 * the libffi CIF in Rust, and is validated at the boundary. See
 * `docs/research/19-den-ffi.md` §3 for why it is data and not a builder.
 *
 * **This module is unsafe by definition.** A schema that does not match the C
 * declaration is undefined behaviour with no diagnostic available at any
 * layer — a `.so` carries no type information, and libffi trusts what it is
 * told. Neither TypeScript nor den can check that `add` really is
 * `int add(int, int)`. den's guarantees stop at: the grant is checked, the
 * schema is validated, disposal is a typed throw rather than a segfault, and
 * numbers that do not fit their declared C type are refused rather than
 * truncated. §5.1 of the design note lists what remains yours.
 *
 * **Implemented today: the whole scalar vocabulary** — every integer width,
 * `bool`, `f32`, `f64`, `pointer` and `void` — as arguments, as results and as
 * static symbols, plus `buffer` arguments, `name`, `optional`, the `ptr`
 * namespace and same-thread `callback`s, all called synchronously. `struct`
 * and `nonblocking` are part of the settled schema but throw
 * `FfiError { kind: "Schema" }` at `open()` until their phase lands.
 */

declare module "den:ffi" {

  /** Marshals as a JS `number`. */
  type NumberType = "i8" | "u8" | "i16" | "u16" | "i32" | "u32" | "f32" | "f64";
  /** Marshals as a JS `bigint` — never a `number`, which cannot hold them. */
  type BigIntType = "i64" | "u64" | "isize" | "usize";

  /** A struct by value. Fields are named, so den computes the ABI padding. */
  interface StructType {
    readonly struct: { readonly [field: string]: ValueType };
  }

  /**
   * `buffer` is a `Uint8Array` whose own bytes — byteOffset included — C
   * borrows for exactly the one synchronous call, so C may write through it and
   * the writes are visible to JS immediately. Three backing stores are refused
   * with `BadArgument` rather than lent out: a detached one, a
   * `SharedArrayBuffer` and a resizable `ArrayBuffer`, either of which another
   * agent could move or rewrite while C holds the address. den passes the
   * address and nothing else: a `length` the C function takes is an ordinary
   * separate argument, and nothing checks that it agrees with the view.
   */
  type ValueType =
    | NumberType
    | BigIntType
    | "bool"
    | "pointer"
    | "buffer"
    | StructType;

  /** `buffer` is in-only: a returned pointer carries no length. */
  type ResultType = Exclude<ValueType, "buffer"> | "void";

  /** An exported variable has the same problem a result does. */
  type StaticType = Exclude<ValueType, "buffer">;

  /**
   * A callback is handed bare addresses: a `buffer` is a length den is *told*,
   * and C tells it nothing, so a callback parameter takes a `pointer` and
   * `ptr.view` instead.
   */
  type CallbackParamType = Exclude<ValueType, "buffer">;

  /** The signature of a JS function C is allowed to call back into. */
  interface FnSig {
    readonly params: readonly CallbackParamType[];
    readonly result: ResultType;
  }

  /** A parameter that takes a `Callback` handle rather than a value. */
  interface CallbackType {
    readonly callback: FnSig;
  }

  type ParamType = ValueType | CallbackType;

  const pointerBrand: unique symbol;

  /**
   * An address, opaque and unforgeable: there is no way to mint one from an
   * integer. It carries the liveness of the library that produced it, so
   * touching it after that library is disposed throws `Closed` — which is *not*
   * a validity check. A pointer the library itself freed still reads live.
   */
  export interface Pointer<T = unknown> {
    readonly [pointerBrand]: T;
  }

  const sigBrand: unique symbol;

  /**
   * A JS function C can call. The brand is load-bearing: without `P` and `R` in
   * the body every handle would satisfy every slot, and a mismatched slot is a
   * wrong-signature libffi call — undefined behaviour reachable from a program
   * that typechecks.
   */
  export interface Callback<
    P extends readonly ValueType[],
    R extends ResultType,
  > extends Disposable {
    readonly pointer: Pointer;
    readonly [sigBrand]: (p: P, r: R) => void;
  }

  /** The JS value one schema type marshals to, in either direction. */
  type Native<T> = T extends NumberType ? number
    : T extends BigIntType ? bigint
    : T extends "bool" ? boolean
    : T extends "pointer" ? Pointer | null
    : T extends "buffer" ? Uint8Array<ArrayBuffer>
    : T extends "void" ? void
    : T extends { callback: infer S }
      ? (S extends FnSig ? Callback<S["params"], S["result"]> : never)
    : T extends { struct: infer F } ? { -readonly [K in keyof F]: Native<F[K]> }
    : never;

  /** An exported function. `nonblocking: true` opts in to the off-thread call. */
  export interface FnDef {
    readonly params: readonly ParamType[];
    readonly result: ResultType;
    readonly nonblocking?: boolean;
    readonly optional?: boolean;
    /** The name to look up, when it is not the key. The key stays the JS
     *  property, so a C identifier that is not a legal one can still be bound. */
    readonly name?: string;
  }

  /** An exported variable, read once at `open()`. */
  export interface StaticDef {
    readonly type: StaticType;
    readonly optional?: boolean;
    /** As on `FnDef`. */
    readonly name?: string;
  }

  type SymbolDef = FnDef | StaticDef;

  /**
   * Note that a per-symbol literal is not freshness-checked — `Schema`'s index
   * signature suppresses it, and the mapped-type fix destroys inference. An
   * unknown key is refused at `open()` instead.
   */
  export type Schema = { readonly [name: string]: SymbolDef };

  type Args<P extends readonly ParamType[]> = {
    -readonly [K in keyof P]: Native<P[K]>;
  };

  type Bound<D> = D extends FnDef
    ? (...args: Args<D["params"]>) =>
      [D["nonblocking"]] extends [true] ? Promise<Native<D["result"]>>
        : Native<D["result"]>
    : D extends StaticDef ? Native<D["type"]>
    : never;

  type Symbols<S extends Schema> = {
    readonly [K in keyof S]: S[K]["optional"] extends true ? Bound<S[K]> | null
      : Bound<S[K]>;
  };

  /**
   * Symbols sit on the handle itself, so the only reserved key is a well-known
   * symbol and a C identifier called `close`, `open` or `read` can never collide
   * with one.
   */
  export type Library<S extends Schema> = Symbols<S> & Disposable;

  /** Which rule was broken. One discriminant, never a sentinel return. */
  export type FfiErrorKind =
    | "NotCapable"
    | "Open"
    | "Symbol"
    | "Schema"
    | "Layout"
    | "Range"
    | "BadArgument"
    | "Closed";

  export class FfiError extends Error {
    private constructor();
    readonly name: "FfiError";
    readonly kind: FfiErrorKind;
    /** The resolved library path, when the failure is about one. */
    readonly path?: string;
    /** The schema key, when the failure is about one. */
    readonly symbol?: string;
  }

  /**
   * The capability. Minted only by the host — `den --allow-ffi[=PATH,...]` — and
   * handed to a realm; there is no constructor, so a script cannot forge one.
   */
  export class FfiGrant {
    private constructor();
    /** Nominal: without a private member a bare object literal would satisfy
     *  the type, and a forgeable grant is not a capability. */
    private readonly roots: unknown;
  }

  /**
   * Mint a C function pointer for `fn`.
   *
   * The handle owns the trampoline, and the signature it carries is checked
   * again — at run time, not only here — against every slot it is passed to.
   *
   * **The lifetime contract is yours.** A `Symbol.dispose` drops the JS
   * function and makes every later JS-side dispatch throw `Closed`, but den
   * keeps the trampoline mapped for the realm's life, because nothing tells it
   * when C stopped holding the address. What den cannot survive is C calling
   * the pointer after the realm itself is gone: that is a jump into unmapped
   * memory, and no layer can turn it into a throw.
   *
   * **Same thread only, for now.** A callback C invokes from a thread it
   * created returns the zero value of its result type and writes one line to
   * stderr; there is no way into a QuickJS realm from a foreign thread until
   * the mailbox lands.
   */
  export function callback<
    const P extends readonly CallbackParamType[],
    const R extends ResultType,
  >(
    def: { readonly params: P; readonly result: R },
    fn: (...args: Args<P>) => Native<R>,
  ): Callback<P, R>;

  /** This realm's grant, or `null` when it was given none. */
  export function grant(): FfiGrant | null;

  /**
   * The platform's shared-library extension — `".so"`, `".dylib"` or `".dll"`,
   * leading dot included — so one specifier can name a library on three
   * operating systems.
   */
  export const suffix: string;

  /**
   * The four things JS may do with an address. There is deliberately no
   * `create(bigint)` and no `offset(p, n)`: a pointer minted from an integer
   * has no owning library, so there would be nothing to check it against.
   */
  export namespace ptr {
    /** The address, for logging and for comparing against C's own idea of it.
     *  It cannot be turned back into a `Pointer`. */
    function value(p: Pointer): bigint;

    /** Address equality. Two `Pointer` objects naming the same address are
     *  distinct JS values, so `===` is not this. */
    function equals(a: Pointer | null, b: Pointer | null): boolean;

    /**
     * A **copy** of `byteLength` bytes at `p`.
     *
     * `byteLength` is mandatory and is **not a bounds check**: a `.so` carries
     * no information about how large a pointee is, so `ptr.view(p, 1 << 30)`
     * is an arbitrary read that passes every check den has. What the length
     * buys is that the returned array has one, so every read *after* this is
     * bounds checked by the engine.
     *
     * The bytes are copied — no JS value ever aliases memory the engine does
     * not own — so writing to the result does not write back to C. Handing a
     * writable region *to* C is the `buffer` parameter type.
     */
    function view(p: Pointer, byteLength: number): Uint8Array<ArrayBuffer>;

    /**
     * A NUL-terminated C string, scanned one byte at a time so that nothing
     * past the terminator is read. No NUL inside the bound throws `Range`,
     * never a silently truncated string. The bound defaults to 4096.
     */
    function cstring(p: Pointer, maxByteLength?: number): string;
  }

  /**
   * Load a shared library and bind every symbol `schema` names. Symbols resolve
   * here, so a missing name throws at `open()` rather than at the first call.
   *
   * `const S` is what removes the need for `as const` on the schema literal.
   */
  export function open<const S extends Schema>(
    path: string,
    schema: S,
    grant: FfiGrant | null,
  ): Library<S>;
}
