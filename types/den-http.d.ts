declare module "den:http" {
  export interface SocketAddr {
    readonly hostname: string;
    readonly port: number;
  }

  export interface ConnectionInfo {
    readonly remote: SocketAddr;
    readonly local: SocketAddr;
  }

  export interface ServeOptions {
    readonly fetch: (
      request: Request,
      connection: ConnectionInfo,
    ) => Response | Promise<Response>;
    readonly listen?: {
      readonly host?: string;
      readonly port?: number;
    };
  }

  export interface Server extends AsyncDisposable {
    readonly addr: SocketAddr;
    readonly url: string;
    readonly finished: Promise<void>;
    readonly pending: {
      readonly requests: number;
      readonly connections: number;
    };
    close(options?: { readonly drainMs?: number }): Promise<void>;
    [Symbol.asyncDispose](): Promise<void>;
  }

  export class HttpError extends Error {
    private constructor();
    readonly kind: "Aborted" | "AddrInUse" | "Bind";
  }

  export function serve(options: ServeOptions): Server;
}
