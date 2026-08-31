/** Durable byte key/value storage backed by SurrealKV. */
declare module "den:kv" {
  type Bytes = Uint8Array<ArrayBufferLike>;

  export class Kv {
    private constructor();

    static open(path: string): Promise<Kv>;

    get(key: Bytes): Promise<Uint8Array<ArrayBuffer> | null>;
    set(key: Bytes, value: Bytes): Promise<void>;
    delete(key: Bytes): Promise<void>;
    transaction(): Promise<KvTransaction>;
    close(): Promise<void>;
  }

  export class KvTransaction {
    private constructor();

    get(key: Bytes): Promise<Uint8Array<ArrayBuffer> | null>;
    getForUpdate(key: Bytes): Promise<Uint8Array<ArrayBuffer> | null>;
    set(key: Bytes, value: Bytes): Promise<void>;
    delete(key: Bytes): Promise<void>;
    /** False means the snapshot conflicted; begin a new transaction. */
    commit(): Promise<boolean>;
    rollback(): Promise<void>;
  }
}
