import bytes from "../blob.bin" with { type: "bytes" };

globalThis.got = [...bytes].join(",");
