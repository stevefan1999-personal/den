import addModule from "./data/add.wasm" with { type: "bytes" };

self.onmessage = async (event: MessageEvent<{ left: number; right: number }>) => {
  const { instance } = await WebAssembly.instantiate(addModule);
  const add = instance.exports.add as (left: number, right: number) => number;
  postMessage(add(event.data.left, event.data.right));
};
