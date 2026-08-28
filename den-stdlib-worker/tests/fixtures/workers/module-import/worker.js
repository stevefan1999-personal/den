import { double } from "./lib.js";
self.onmessage = (event) => postMessage(double(event.data));
