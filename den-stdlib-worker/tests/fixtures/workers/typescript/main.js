const classic = new Worker("./classic.ts");
const asModule = new Worker("./module.ts", { type: "module" });
classic.postMessage({ value: 1 });
asModule.postMessage({ value: 2 });
globalThis.result = [
  await firstMessage(classic),
  await firstMessage(asModule),
].join(",");
classic.terminate();
asModule.terminate();
