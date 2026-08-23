// WHATWG File API File. Ported from txiki.js polyfills/file.js.
(function (natives, api) {
  const { Blob } = api;
  const { now } = Date;
  const { isNaN } = Number;

  class File extends Blob {
    #lastModified = 0;
    #name = "";

    constructor(fileBits, fileName, options = {}) {
      if (arguments.length < 2) {
        throw new TypeError(
          `Failed to construct 'File': 2 arguments required, but only ${arguments.length} present.`,
        );
      }
      super(fileBits, options);
      const lastModified = options.lastModified === undefined
        ? now()
        : Number(options.lastModified);
      if (!isNaN(lastModified)) this.#lastModified = lastModified;
      this.#name = String(fileName);
    }

    get name() {
      return this.#name;
    }

    get lastModified() {
      return this.#lastModified;
    }

    get [Symbol.toStringTag]() {
      return "File";
    }
  }

  Object.defineProperties(File.prototype, {
    name: { enumerable: true },
    lastModified: { enumerable: true },
  });

  return { ...api, File };
})
