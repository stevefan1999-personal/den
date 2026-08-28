import { assert, assertEquals } from "den:assert";
import { firstMessage } from "../lib/worker.js";

const worker = new Worker("./worker.js");
const reply = firstMessage(worker);
const before = Temporal.Now.instant().epochMilliseconds;
worker.postMessage(25);
const data = await reply;
worker.terminate();

assertEquals(data.delay, 25);
assert(data.epoch >= before);
