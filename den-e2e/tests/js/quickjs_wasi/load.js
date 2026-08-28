export const PACKAGE = "3.0.0";

export async function compileQuickJS() {
  const api = await import(`https://esm.sh/quickjs-wasi@${PACKAGE}`);
  const wasm = await WebAssembly.compileStreaming(
    fetch(`https://cdn.jsdelivr.net/npm/quickjs-wasi@${PACKAGE}/quickjs.wasm`),
  );
  return { ...api, wasm };
}

export async function loadQuickJS(options = {}) {
  const compiled = await compileQuickJS();
  const vm = await compiled.QuickJS.create({ wasm: compiled.wasm, ...options });
  return { ...compiled, vm };
}

export async function settle(vm, handle) {
  while (handle.promiseState === 0) {
    if (vm.executePendingJobs() === 0) break;
  }
  return vm.resolvePromise(handle);
}
