// CompressionStream / DecompressionStream. JS TransformStream wrapping
// flate2 natives, after txiki.js polyfills/compression-streams.js.
(function (natives, api) {
  const { TransformStream } = api;
  const MZ_NO_FLUSH = 0;
  const MZ_FINISH = 4;
  const validFormats = ["gzip", "deflate", "deflate-raw"];

  const toUint8Array = (chunk) => {
    if (chunk instanceof Uint8Array) return chunk;
    if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
    if (ArrayBuffer.isView(chunk)) {
      return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
    }
    throw new TypeError("chunk must be a BufferSource");
  };

  class CompressionStream {
    constructor(format) {
      if (!validFormats.includes(format)) {
        throw new TypeError(`Unsupported compression format: '${format}'`);
      }
      const compressor = new natives.Compressor(format);
      const { readable, writable } = new TransformStream({
        transform(chunk, controller) {
          const result = compressor.process(toUint8Array(chunk), MZ_NO_FLUSH);
          if (result.length > 0) controller.enqueue(result);
        },
        flush(controller) {
          const result = compressor.process(new Uint8Array(0), MZ_FINISH);
          if (result.length > 0) controller.enqueue(result);
        },
      });
      this.readable = readable;
      this.writable = writable;
    }

    get [Symbol.toStringTag]() {
      return "CompressionStream";
    }
  }

  class DecompressionStream {
    constructor(format) {
      if (!validFormats.includes(format)) {
        throw new TypeError(`Unsupported compression format: '${format}'`);
      }
      const decompressor = new natives.Decompressor(format);
      const { readable, writable } = new TransformStream({
        transform(chunk, controller) {
          const result = decompressor.process(toUint8Array(chunk));
          if (result.length > 0) controller.enqueue(result);
        },
      });
      this.readable = readable;
      this.writable = writable;
    }

    get [Symbol.toStringTag]() {
      return "DecompressionStream";
    }
  }

  return { ...api, CompressionStream, DecompressionStream };
})
