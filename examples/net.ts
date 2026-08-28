export type ByteSource = string | Uint8Array;

export interface ByteStream {
  read(bytes: number): Promise<Uint8Array>;
  writeAll?(data: ByteSource): Promise<void>;
  write_all?(data: ByteSource): Promise<void>;
  shutdown(): Promise<void>;
}

export interface SocketAddr {
  toString(): string;
}

export interface Listener {
  localAddr?: SocketAddr;
  local_addr?: SocketAddr;
  accept(): Promise<[ByteStream, SocketAddr]>;
}

export function writeAll(stream: ByteStream, data: ByteSource): Promise<void> {
  const write = stream.writeAll ?? stream.write_all;
  if (write === undefined) {
    throw new TypeError("stream has no writeAll");
  }
  return write.call(stream, data);
}

export function dest(listener: Listener): string {
  const addr = listener.localAddr ?? listener.local_addr;
  if (addr === undefined) {
    throw new TypeError("listener has no local address");
  }
  return addr.toString();
}

export async function* connections(
  listener: Listener,
): AsyncGenerator<{ stream: ByteStream; peer: SocketAddr }> {
  for (;;) {
    const [stream, peer] = await listener.accept();
    yield { stream, peer };
  }
}
