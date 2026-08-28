import { label } from "./lib.ts";
enum Kind { Module = "module" }
self.onmessage = (event: MessageEvent): void => {
  postMessage(`${Kind.Module}:${label(event.data.value)}`);
};
