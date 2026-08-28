import { double } from "./worker_double.js";

self.onmessage = (event) => postMessage(double(event.data));
