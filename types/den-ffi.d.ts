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
 * **Implemented today (phase 1): `i32`, `f64` and `void`, called
 * synchronously.** Every other member of `ValueType`, and the `nonblocking`,
 * `optional`, `type`, `struct` and `callback` forms, is part of the settled
 * schema but throws `FfiError { kind: "Schema" }` at `open()` until its phase
 * lands. `Callback<P, R>` is declared here from the start, brand and all,
 * because adding a brand to a published interface later is a breaking change.
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

  type ValueType =
    | NumberType
    | BigIntType
    | "bool"
    | "pointer"
    | "buffer"
    | StructType;

  /** `buffer` is in-only: a returned pointer carries no length. */
  type ResultType = Exclude<ValueType, "buffer"> | "void";

  /** The signature of a JS function C is allowed to call back into. */
  interface FnSig {
    readonly params: readonly ValueType[];
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
  }

  /** An exported variable. */
  export interface StaticDef {
    readonly type: ValueType;
    readonly optional?: boolean;
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

  /** This realm's grant, or `null` when it was given none. */
  export function grant(): FfiGrant | null;

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
