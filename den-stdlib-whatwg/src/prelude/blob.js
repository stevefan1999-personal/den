// WHATWG File API Blob. Ported from txiki.js polyfills/blob.js and adapted
// to den's prelude bag; stream() uses this crate's ReadableStream.
(function (natives, api) {
  const { ReadableStream } = api;
  const POOL_SIZE = 65536;
  const { isView } = ArrayBuffer;

  const encodeText = (value) => {
    const text = `${value}`;
    if (typeof TextEncoder === "function") {
      return new TextEncoder().encode(text);
    }
    const encoded = unescape(encodeURIComponent(text));
    const bytes = new Uint8Array(encoded.length);
    for (let offset = 0; offset < encoded.length; offset++) {
      bytes[offset] = encoded.charCodeAt(offset) & 0xff;
    }
    return bytes;
  };

  const decodeText = (buffer) => {
    if (typeof TextDecoder === "function") {
      return new TextDecoder().decode(buffer);
    }
    const bytes = new Uint8Array(buffer);
    let binary = "";
    for (let offset = 0; offset < bytes.length; offset++) {
      binary += String.fromCharCode(bytes[offset]);
    }
    return decodeURIComponent(escape(binary));
  };

  async function* toIterator(parts) {
    for (const part of parts) {
      if (isView(part)) {
        let position = part.byteOffset;
        const end = part.byteOffset + part.byteLength;
        while (position !== end) {
          const size = Math.min(end - position, POOL_SIZE);
          const chunk = part.buffer.slice(position, position + size);
          position += chunk.byteLength;
          yield new Uint8Array(chunk);
        }
      } else {
        yield* part.stream();
      }
    }
  }

  class Blob {
    #parts = [];
    #type = "";
    #size = 0;

    constructor(blobParts = [], options = {}) {
      if (typeof blobParts !== "object" || blobParts === null) {
        throw new TypeError(
          "Failed to construct 'Blob': The provided value cannot be converted to a sequence.",
        );
      }
      if (typeof blobParts[Symbol.iterator] !== "function") {
        throw new TypeError(
          "Failed to construct 'Blob': The object must have a callable @@iterator property.",
        );
      }
      if (typeof options !== "object" && typeof options !== "function") {
        throw new TypeError(
          "Failed to construct 'Blob': parameter 2 cannot convert to dictionary.",
        );
      }
      if (options === null) options = {};

      for (const element of blobParts) {
        let part;
        if (isView(element)) {
          part = new Uint8Array(
            element.buffer.slice(
              element.byteOffset,
              element.byteOffset + element.byteLength,
            ),
          );
        } else if (element instanceof ArrayBuffer) {
          part = new Uint8Array(element.slice(0));
        } else if (element instanceof Blob) {
          part = element;
        } else {
          part = encodeText(element);
        }
        const size = isView(part) ? part.byteLength : part.size;
        if (size) {
          this.#size += size;
          this.#parts.push(part);
        }
      }

      const type = options.type === undefined ? "" : String(options.type);
      this.#type = /^[\x20-\x7E]*$/.test(type) ? type : "";
    }

    get size() {
      return this.#size;
    }

    get type() {
      return this.#type;
    }

    async text() {
      const buffer = await this.arrayBuffer();
      return decodeText(buffer);
    }

    async arrayBuffer() {
      const data = new Uint8Array(this.size);
      let offset = 0;
      for await (const chunk of toIterator(this.#parts)) {
        data.set(chunk, offset);
        offset += chunk.length;
      }
      return data.buffer;
    }

    stream() {
      const iterator = toIterator(this.#parts);
      return new ReadableStream({
        async pull(controller) {
          const chunk = await iterator.next();
          chunk.done ? controller.close() : controller.enqueue(chunk.value);
        },
        async cancel() {
          await iterator.return?.();
        },
      });
    }

    slice(start = 0, end = this.size, type = "") {
      const { size } = this;
      let relativeStart = start < 0 ? Math.max(size + start, 0) : Math.min(start, size);
      let relativeEnd = end < 0 ? Math.max(size + end, 0) : Math.min(end, size);
      const span = Math.max(relativeEnd - relativeStart, 0);
      const blobParts = [];
      let added = 0;

      for (const part of this.#parts) {
        if (added >= span) break;
        const partSize = isView(part) ? part.byteLength : part.size;
        if (relativeStart && partSize <= relativeStart) {
          relativeStart -= partSize;
          relativeEnd -= partSize;
        } else {
          let chunk;
          if (isView(part)) {
            chunk = part.subarray(relativeStart, Math.min(partSize, relativeEnd));
            added += chunk.byteLength;
          } else {
            chunk = part.slice(relativeStart, Math.min(partSize, relativeEnd));
            added += chunk.size;
          }
          relativeEnd -= partSize;
          blobParts.push(chunk);
          relativeStart = 0;
        }
      }

      const blob = new Blob([], { type: `${type}` });
      blob.#size = span;
      blob.#parts = blobParts;
      return blob;
    }

    get [Symbol.toStringTag]() {
      return "Blob";
    }
  }

  Object.defineProperties(Blob.prototype, {
    size: { enumerable: true },
    type: { enumerable: true },
    slice: { enumerable: true },
  });

  return { ...api, Blob };
})
