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

namespace JSX {
  interface IntrinsicElements {
    [elemName: string]: Record<string, unknown>;
  }
}

declare module "den:assert" {
  export function assert(value: unknown, message?: string): void;
  export function assertEquals(actual: unknown, expected: unknown, message?: string): void;
}

declare module "den:process" {
  export const env: Record<string, string | undefined>;
  export function cwd(): string;
  export function exit(code?: number): never;
  export function addSignalListener(signal: string, listener: () => void): void;
}

declare module "den:sqlite" {
  export class Connection {
    static open(path: string): Connection;
    static openInMemory?: () => Connection;
    static open_in_memory?: () => Connection;
    execute(sql: string, params?: unknown[]): number;
    queryRows?(sql: string, params?: unknown[]): unknown[][] | null;
    query_rows?(sql: string, params?: unknown[]): unknown[][] | null;
    close(): void;
  }
}

declare module "den:networking" {
  export class SocketAddr {
    readonly port: number;
    readonly is_ipv4: boolean;
    readonly is_ipv6: boolean;
    toString(): string;
  }

  export class TcpStream {
    static connect(addr: string): Promise<TcpStream>;
    read(bytes: number): Promise<Uint8Array>;
    writeAll?(data: string | Uint8Array): Promise<void>;
    write_all?(data: string | Uint8Array): Promise<void>;
    shutdown(): Promise<void>;
  }

  export class TcpListener {
    static listen(addr: string): Promise<TcpListener>;
    readonly localAddr?: SocketAddr;
    readonly local_addr?: SocketAddr;
    accept(): Promise<[TcpStream, SocketAddr]>;
  }

  export class TlsStream {
    static connect(addr: string, domain: string, caPem?: string): Promise<TlsStream>;
    read(bytes: number): Promise<Uint8Array>;
    writeAll?(data: string | Uint8Array): Promise<void>;
    write_all?(data: string | Uint8Array): Promise<void>;
    shutdown(): Promise<void>;
  }

  export class TlsListener {
    static listen(addr: string, certPem: string, keyPem: string): Promise<TlsListener>;
    readonly localAddr?: SocketAddr;
    readonly local_addr?: SocketAddr;
    accept(): Promise<[TlsStream, SocketAddr]>;
  }
}
