/** Ambient shapes for `den:*` modules used by the examples. Oxc strips these. */

declare module "https://esm.sh/react@18.3.1" {
  const React: {
    createElement: (...args: unknown[]) => unknown;
    Fragment: symbol;
  };
  export default React;
}

declare module "https://esm.sh/react-dom@18.3.1/server" {
  export function renderToStaticMarkup(node: unknown): string;
}

declare module "*.json" {
  const value: Record<string, unknown>;
  export default value;
}

declare module "*.txt" {
  const value: string;
  export default value;
}

declare module "*.wasm" {
  const value: Uint8Array;
  export default value;
}

declare module "greet" {
  export function greet(name: string): string;
}

namespace JSX {
  interface IntrinsicElements {
    [elemName: string]: Record<string, unknown>;
  }
}

interface GPUAdapter {
  requestDevice(descriptor?: Record<string, unknown>): Promise<GPUDevice>;
}

interface GPUDevice {
  createBuffer(descriptor: Record<string, unknown>): GPUBuffer;
  createShaderModule(descriptor: { code: string }): GPUShaderModule;
  createComputePipeline(descriptor: Record<string, unknown>): GPUComputePipeline;
  createBindGroup(descriptor: Record<string, unknown>): GPUBindGroup;
  createCommandEncoder(): GPUCommandEncoder;
  pushErrorScope(filter: string): void;
  popErrorScope(): Promise<unknown>;
  queue: { submit(commands: unknown[]): void };
  destroy(): void;
}

interface GPUBuffer {
  getMappedRange(): ArrayBuffer;
  unmap(): void;
  mapAsync(mode: number): Promise<void>;
  destroy(): void;
}

interface GPUShaderModule {}

interface GPUComputePipeline {
  getBindGroupLayout(index: number): unknown;
}

interface GPUBindGroup {}

interface GPUCommandEncoder {
  beginComputePass(): GPUComputePassEncoder;
  copyBufferToBuffer(
    source: GPUBuffer,
    sourceOffset: number,
    destination: GPUBuffer,
    destinationOffset: number,
    size: number,
  ): void;
  finish(): unknown;
}

interface GPUComputePassEncoder {
  setPipeline(pipeline: GPUComputePipeline): void;
  setBindGroup(index: number, group: GPUBindGroup): void;
  dispatchWorkgroups(x: number, y?: number, z?: number): void;
  end(): void;
}

declare namespace Temporal {
  namespace Now {
    function instant(): Instant;
    function plainDateISO(timeZone?: string): PlainDate;
    function zonedDateTimeISO(timeZone?: string): { timeZoneId: string; offset: string };
    function timeZoneId(): string;
  }

  class Instant {
    toString(): string;
  }

  class PlainDate {
    static from(input: string): PlainDate;
    readonly year: number;
    add(duration: { days?: number; months?: number }): PlainDate;
    subtract(duration: { days?: number }): PlainDate;
    toString(): string;
  }

  class Duration {
    constructor(years?: number, months?: number, weeks?: number, days?: number);
    static from(input: { days?: number; hours?: number }): Duration;
    readonly days: number;
    toString(): string;
  }
}

declare module "den:assert" {
  export function assert(value: unknown, message?: string): void;
  export function assertEquals(actual: unknown, expected: unknown, message?: string): void;
}

declare module "den:process" {
  export interface ChildStatus {
    readonly code: number | null;
  }

  export interface PipeReader {
    text(): Promise<string>;
  }

  export interface ChildProcess {
    readonly pid: number;
    readonly stdout: PipeReader | null;
    readonly stderr: PipeReader | null;
    wait(): Promise<ChildStatus>;
    kill(signal?: string): void;
  }

  export interface LookupAddress {
    readonly family: 4 | 6;
    readonly ip: string;
  }

  export const env: Record<string, string | undefined>;
  export const argv: string[];
  export const pid: number;
  export function cwd(): string;
  export function exit(code?: number): never;
  export function addSignalListener(signal: string, listener: () => void): void;
  export function spawn(
    cmd: string | string[],
    options?: {
      cwd?: string;
      env?: Record<string, string>;
      stdin?: "pipe" | "ignore" | "inherit";
      stdout?: "pipe" | "ignore" | "inherit";
      stderr?: "pipe" | "ignore" | "inherit";
    },
  ): ChildProcess;
  export function lookup(
    host: string,
    options?: { all?: boolean; family?: number },
  ): Promise<LookupAddress | LookupAddress[]>;
}

declare module "den:sqlite" {
  export class Connection {
    static open(path: string): Connection;
    static open_in_memory(): Connection;
    execute(sql: string, params?: unknown[]): number;
    query_rows(sql: string, params?: unknown[]): unknown[][] | null;
    close(): void;
  }
}

declare module "den:fs" {
  export interface FsStat {
    readonly len: number | bigint;
    readonly isFile: boolean;
    readonly isDir: boolean;
    readonly isSymlink: boolean;
    readonly mode?: number;
  }

  export interface DirEntry {
    readonly name: string;
    readonly isFile: boolean;
    readonly isDir: boolean;
    readonly isSymlink: boolean;
  }

  export function canonicalize(path: string): Promise<string | null>;
  export function copy(from: string, to: string): Promise<void>;
  export function createDir(path: string): Promise<void>;
  export function createDirAll(path: string): Promise<void>;
  export function metadata(path: string): Promise<FsStat>;
  export function read(path: string): Promise<number[]>;
  export function readDir(path: string): Promise<DirEntry[]>;
  export function readToString(path: string): Promise<string>;
  export function removeDir(path: string): Promise<void>;
  export function removeDirAll(path: string): Promise<void>;
  export function removeFile(path: string): Promise<void>;
  export function rename(from: string, to: string): Promise<void>;
  export function write(
    path: string,
    contents: number[],
    options?: { atomic?: boolean },
  ): Promise<void>;
}

declare module "den:path" {
  export interface PathApi {
    basename(path: string, suffix?: string): string;
    dirname(path: string): string;
    extname(path: string): string;
    join(...parts: string[]): string;
    normalize(path: string): string;
    isAbsolute(path: string): boolean;
    relative(from: string, to: string): string;
    resolve(...parts: string[]): string;
    parse(path: string): {
      root: string;
      dir: string;
      base: string;
      ext: string;
      name: string;
    };
    posix: PathApi;
    windows: PathApi;
    sep: string;
    delimiter: string;
  }

  const path: PathApi;
  export default path;
  export const posix: PathApi;
  export const windows: PathApi;
  export const sep: string;
  export const delimiter: string;
  export function join(...parts: string[]): string;
  export function basename(path: string, suffix?: string): string;
  export function dirname(path: string): string;
}

declare module "den:webgpu" {
  export const gpu: { requestAdapter(): Promise<GPUAdapter | null> };
  export const GPUBufferUsage: {
    STORAGE: number;
    COPY_SRC: number;
    COPY_DST: number;
    MAP_READ: number;
    MAP_WRITE: number;
  };
  export const GPUMapMode: { READ: number; WRITE: number };
}

declare module "den:networking" {
  export class IpAddr {
    readonly is_loopback: boolean;
    readonly is_ipv4: boolean;
    readonly is_ipv6: boolean;
    toString(): string;
  }

  export class SocketAddr {
    readonly port: number;
    readonly is_ipv4: boolean;
    readonly is_ipv6: boolean;
    readonly ip: IpAddr;
    toString(): string;
  }

  export class TcpStream {
    static connect(addr: string): Promise<TcpStream>;
    read(bytes: number): Promise<Uint8Array>;
    writeAll(data: string | Uint8Array): Promise<void>;
    write_all(data: string | Uint8Array): Promise<void>;
    shutdown(): Promise<void>;
  }

  export class TcpListener {
    static listen(addr: string): Promise<TcpListener>;
    readonly localAddr: SocketAddr;
    readonly local_addr: SocketAddr;
    accept(): Promise<[TcpStream, SocketAddr]>;
  }

  export class TlsStream {
    static connect(addr: string, domain: string, caPem?: string): Promise<TlsStream>;
    read(bytes: number): Promise<Uint8Array>;
    writeAll(data: string | Uint8Array): Promise<void>;
    write_all(data: string | Uint8Array): Promise<void>;
    shutdown(): Promise<void>;
  }

  export class TlsListener {
    static listen(addr: string, certPem: string, keyPem: string): Promise<TlsListener>;
    readonly localAddr: SocketAddr;
    readonly local_addr: SocketAddr;
    accept(): Promise<[TlsStream, SocketAddr]>;
  }

  export class UdpSocket {
    static bind(addr: string): Promise<UdpSocket>;
    readonly localAddr: SocketAddr;
    sendTo(buf: string | Uint8Array, addr: string): Promise<number>;
    recvFrom(max: number): Promise<[Uint8Array, SocketAddr]>;
  }

  export class UnixStream {
    static connect(path: string): Promise<UnixStream>;
    read(bytes: number): Promise<Uint8Array>;
    writeAll(data: string | Uint8Array): Promise<void>;
    write_all(data: string | Uint8Array): Promise<void>;
    shutdown(): Promise<void>;
  }

  export class UnixListener {
    static listen(path: string): Promise<UnixListener>;
    readonly localAddr: string;
    readonly local_addr: string;
    accept(): Promise<[UnixStream, string]>;
  }
}
