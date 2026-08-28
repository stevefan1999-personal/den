import data from "../data.json" with { type: "json" };

globalThis.got = `${data.foo}:${data.n}`;
