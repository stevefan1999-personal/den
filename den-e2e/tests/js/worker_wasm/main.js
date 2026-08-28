import { assertEquals } from "den:assert";
import { firstMessage } from "../lib/worker.js";

const worker = new Worker("./worker.js");
const reply = firstMessage(worker);
worker.postMessage({ left: 40, right: 2 });
assertEquals(await reply, 42);
worker.terminate();
