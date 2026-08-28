import { assertEquals } from "den:assert";
import { firstMessage } from "../lib/worker.js";

const worker = new Worker("./worker.js");
const reply = firstMessage(worker);
worker.postMessage("https://cdn.jsdelivr.net/npm/ms@2.1.3/package.json");
const data = await reply;
worker.terminate();

if (!data?.ok) {
  throw new Error(data?.message ?? "worker fetch failed");
}
const pkg = JSON.parse(data.text);
assertEquals(pkg.name, "ms");
