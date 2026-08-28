const buffer = new Uint8Array([1, 2, 3, 4]).buffer;
const sent = {
  primitives: [undefined, null, true, -0, NaN, Infinity, 1.5, "text"],
  big: 9007199254740993n,
  date: new Date(86400000),
  regexp: /ab+c/gi,
  map: new Map([["key", { nested: 1 }]]),
  set: new Set([1, "two"]),
  error: new TypeError("typed"),
  domException: new DOMException("denied", "NotAllowedError"),
  buffer,
  view: new Uint16Array(buffer, 2, 1),
  dataView: new DataView(buffer, 1, 2),
  holes: [1, , 3],
  nested: { deep: { deeper: [1, [2, [3]]] } },
};
// A cycle: the writer's reference table has to survive the trip.
sent.self = sent;

const worker = new Worker("./worker.js");
worker.postMessage(sent);
const back = await firstMessage(worker);
worker.terminate();

globalThis.result = [
  `primitives:${back.primitives.map((value) => `${value}`).join("|")}`,
  `negativeZero:${Object.is(back.primitives[3], -0)}`,
  `big:${back.big}:${typeof back.big}`,
  `date:${back.date instanceof Date}:${back.date.getTime()}`,
  `regexp:${back.regexp.source}:${back.regexp.flags}:${back.regexp.lastIndex}`,
  `map:${back.map instanceof Map}:${back.map.get("key").nested}`,
  `set:${back.set instanceof Set}:${[...back.set].join("|")}`,
  `error:${back.error instanceof TypeError}:${back.error.message}`,
  `domException:${back.domException.name}:${back.domException.message}`,
  `buffer:${new Uint8Array(back.buffer).join("|")}`,
  `view:${back.view instanceof Uint16Array}:${back.view.byteOffset}:${back.view.length}`,
  `dataView:${back.dataView.byteOffset}:${back.dataView.byteLength}`,
  // v1 divergence (docs/research/10 §4.5): a hole arrives as undefined,
  // i.e. as a present property.
  `holes:${back.holes.length}:${back.holes[1]}:${1 in back.holes}`,
  `nested:${back.nested.deep.deeper[1][1][0]}`,
  `cycle:${back.self === back}`,
  // Aliasing inside one message is preserved; the buffer was cloned
  // rather than transferred, so this side still owns its own.
  `aliased:${back.view.buffer === back.buffer}`,
  `detached:${buffer.detached}`,
].join("\n");
