// FormData. Adapted from formdata-polyfill (MIT, Jimmy Wärting) via txiki.js.
// No HTMLFormElement constructor: den has no DOM. Multipart serialisation is
// reachable through Symbol.for("den.toMultipartBlob") so fetch/XHR can send
// File parts as raw bytes without this crate depending on the fetch crate.
(function (natives, api) {
  const { Blob, File } = api;
  const TO_MULTIPART = Symbol.for("den.toMultipartBlob");

  const ensureArgs = (args, expected) => {
    if (args.length < expected) {
      throw new TypeError(
        `${expected} argument required, but only ${args.length} present.`,
      );
    }
  };

  const normalizeArgs = (name, value, filename) => {
    if (value instanceof Blob) {
      filename = filename !== undefined
        ? String(filename)
        : typeof value.name === "string"
        ? value.name
        : "blob";
      if (value.name !== filename || Object.prototype.toString.call(value) === "[object Blob]") {
        value = new File([value], filename, { type: value.type });
      }
      return [String(name), value];
    }
    return [String(name), String(value)];
  };

  const normalizeLinefeeds = (value) => value.replace(/\r?\n|\r/g, "\r\n");
  const escape = (str) =>
    str.replace(/\n/g, "%0A").replace(/\r/g, "%0D").replace(/"/g, "%22");

  class FormData {
    #data = [];

    constructor(form) {
      if (form !== undefined && form !== null) {
        throw new TypeError("Failed to construct 'FormData': HTMLFormElement is not supported");
      }
    }

    append(name, value, filename) {
      ensureArgs(arguments, 2);
      this.#data.push(normalizeArgs(name, value, filename));
    }

    delete(name) {
      ensureArgs(arguments, 1);
      name = String(name);
      this.#data = this.#data.filter((entry) => entry[0] !== name);
    }

    *entries() {
      yield* this.#data;
    }

    forEach(callback, thisArg) {
      ensureArgs(arguments, 1);
      for (const [name, value] of this) {
        callback.call(thisArg, value, name, this);
      }
    }

    get(name) {
      ensureArgs(arguments, 1);
      name = String(name);
      for (const entry of this.#data) {
        if (entry[0] === name) return entry[1];
      }
      return null;
    }

    getAll(name) {
      ensureArgs(arguments, 1);
      name = String(name);
      const result = [];
      for (const entry of this.#data) {
        if (entry[0] === name) result.push(entry[1]);
      }
      return result;
    }

    has(name) {
      ensureArgs(arguments, 1);
      name = String(name);
      return this.#data.some((entry) => entry[0] === name);
    }

    *keys() {
      for (const [name] of this) yield name;
    }

    set(name, value, filename) {
      ensureArgs(arguments, 2);
      name = String(name);
      const args = normalizeArgs(name, value, filename);
      const result = [];
      let replace = true;
      for (const entry of this.#data) {
        if (entry[0] === name) {
          if (replace) {
            result.push(args);
            replace = false;
          }
        } else {
          result.push(entry);
        }
      }
      if (replace) result.push(args);
      this.#data = result;
    }

    *values() {
      for (const [, value] of this) yield value;
    }

    #blob() {
      const boundary = "----formdata-den-" + Math.random().toString(16).slice(2);
      const chunks = [];
      const prefix = `--${boundary}\r\nContent-Disposition: form-data; name="`;
      this.forEach((value, name) => {
        if (typeof value === "string") {
          chunks.push(
            prefix + escape(normalizeLinefeeds(name)) +
              `"\r\n\r\n${normalizeLinefeeds(value)}\r\n`,
          );
        } else {
          chunks.push(
            prefix + escape(normalizeLinefeeds(name)) +
              `"; filename="${escape(value.name)}"\r\nContent-Type: ${
                value.type || "application/octet-stream"
              }\r\n\r\n`,
            value,
            `\r\n`,
          );
        }
      });
      chunks.push(`--${boundary}--`);
      return new Blob(chunks, {
        type: "multipart/form-data; boundary=" + boundary,
      });
    }

    [Symbol.iterator]() {
      return this.entries();
    }

    [TO_MULTIPART]() {
      return this.#blob();
    }

    get [Symbol.toStringTag]() {
      return "FormData";
    }
  }

  return { ...api, FormData };
})
