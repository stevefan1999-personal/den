import { assert, assertEquals, assertRejects, assertThrows } from "den:assert";
import { GPUBufferUsage, GPUMapMode, gpu } from "den:webgpu";

// Every interface the module exports is the very same global constructor.
const exported = await import("den:webgpu");
for (const name of [
  "GPU", "GPUAdapter", "GPUAdapterInfo", "GPUBindGroup", "GPUBindGroupLayout",
  "GPUBuffer", "GPUCommandBuffer", "GPUCommandEncoder", "GPUComputePassEncoder",
  "GPUComputePipeline", "GPUDevice", "GPUDeviceLostInfo", "GPUError",
  "GPUExternalTexture", "GPUInternalError", "GPUOutOfMemoryError",
  "GPUPipelineError", "GPUPipelineLayout", "GPUQuerySet", "GPUQueue",
  "GPURenderBundle", "GPURenderBundleEncoder", "GPURenderPassEncoder",
  "GPURenderPipeline", "GPUSampler", "GPUShaderModule", "GPUSupportedFeatures",
  "GPUSupportedLimits", "GPUSupportedWGSLLanguageFeatures", "GPUTexture",
  "GPUTextureView", "GPUUncapturedErrorEvent", "GPUValidationError",
]) {
  assertEquals(typeof globalThis[name], "function", name);
  assert(exported[name] === globalThis[name], name);
}

// The setlike interfaces: what script can observe on them.
const adapter = await gpu.requestAdapter();
assert(adapter !== null);
for (const [set, Ctor] of [
  [adapter.features, GPUSupportedFeatures],
  [gpu.wgslLanguageFeatures, GPUSupportedWGSLLanguageFeatures],
]) {
  assert(set instanceof Ctor);
  assertEquals(typeof set.size, "number");
  assertEquals([...set].length, set.size);
  assertEquals([...set.keys()].join(), [...set.values()].join());
  assert([...set.entries()].every(([key, value]) => key === value));
  let seen = 0;
  set.forEach(function (value, key, self) {
    assert(value === key && self === set && this === adapter);
    seen += 1;
  }, adapter);
  assertEquals(seen, set.size);
  assertEquals(set.has("not-a-feature"), false);
  assertThrows(() => new Ctor(), TypeError, "Illegal constructor");
}
assert(adapter.features.has("core-features-and-limits"));

// requiredFeatures takes any iterable of names, the adapter's own set included,
// and an unknown name is a TypeError.
for (const required of [
  adapter.features,
  [...adapter.features],
  new Set(adapter.features),
  undefined,
]) {
  const device = await adapter.requestDevice({ requiredFeatures: required });
  assert(device.features.has("core-features-and-limits"));
  assert(device.features instanceof GPUSupportedFeatures);
  device.destroy();
}
await assertRejects(
  () => adapter.requestDevice({ requiredFeatures: ["not-a-feature"] }),
  TypeError,
  "unknown WebGPU feature not-a-feature",
);

// The per-stage limit names alias the per-shader-stage fields, on read and
// in requiredLimits alike; an unknown limit name is an OperationError.
const device = await adapter.requestDevice();
const limits = device.limits;
assertEquals(limits.maxStorageBuffersInVertexStage, limits.maxStorageBuffersPerShaderStage);
assertEquals(limits.maxStorageBuffersInFragmentStage, limits.maxStorageBuffersPerShaderStage);
assertEquals(limits.maxStorageTexturesInVertexStage, limits.maxStorageTexturesPerShaderStage);
assertEquals(limits.maxStorageTexturesInFragmentStage, limits.maxStorageTexturesPerShaderStage);
const aliased = await adapter.requestDevice({ requiredLimits: { maxStorageBuffersInVertexStage: 1 } });
assert(aliased.limits instanceof GPUSupportedLimits);
aliased.destroy();
const unknown = await assertRejects(
  () => adapter.requestDevice({ requiredLimits: { maxBogus: 1 } }),
  DOMException,
  "unknown required limit maxBogus",
);
assertEquals(unknown.name, "OperationError");

// writeBuffer counts dataOffset/size in elements of the source view, so the
// element width of every view kind is observable in the bytes that land.
const sink = device.createBuffer({
  size: 32,
  usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
});
device.queue.writeBuffer(sink, 0, new Uint16Array([1, 2, 3, 4, 5, 6]), 2, 4);
device.queue.writeBuffer(sink, 8, new Float64Array([1.5, 2.5, 3.5]), 1, 1);
device.queue.writeBuffer(sink, 16, new DataView(new Uint8Array([9, 8, 7, 6]).buffer), 0, 4);
device.queue.writeBuffer(sink, 20, new Uint8Array([1, 2, 3, 4, 5]).buffer, 1, 4);
device.queue.writeBuffer(sink, 24, new Int32Array([-1, -2, -3]), 1);
await sink.mapAsync(GPUMapMode.READ);
const landed = sink.getMappedRange();
assertEquals([...new Uint16Array(landed, 0, 4)], [3, 4, 5, 6]);
assertEquals(new Float64Array(landed, 8, 1)[0], 2.5);
assertEquals([...new Uint8Array(landed, 16, 4)], [9, 8, 7, 6]);
assertEquals([...new Uint8Array(landed, 20, 4)], [2, 3, 4, 5]);
assertEquals([...new Int32Array(landed, 24, 2)], [-2, -3]);
sink.unmap();
assertThrows(
  () => device.queue.writeBuffer(sink, 0, new Uint16Array([1, 2, 3]), 0, 3),
  DOMException,
  "multiple of 4",
);

// The error hierarchy: one-argument constructors, an enumerable `message`
// accessor on each prototype, and the three subclasses extend GPUError.
for (const Ctor of [GPUError, GPUValidationError, GPUOutOfMemoryError, GPUInternalError]) {
  assertEquals(Ctor.length, 1);
  const error = new Ctor("why");
  assert(error instanceof GPUError);
  assertEquals(error.message, "why");
  assertEquals(Object.getPrototypeOf(error), Ctor.prototype);
  const descriptor = Object.getOwnPropertyDescriptor(Ctor.prototype, "message");
  assert(descriptor.enumerable && typeof descriptor.get === "function");
}
for (const Sub of [GPUValidationError, GPUOutOfMemoryError, GPUInternalError]) {
  assertEquals(Object.getPrototypeOf(Sub.prototype), GPUError.prototype);
}

// GPUPipelineError is a DOMException named after itself, carrying `reason`.
assertEquals(GPUPipelineError.length, 2);
const pipelineError = new GPUPipelineError("bad", { reason: "internal" });
assert(pipelineError instanceof GPUPipelineError);
assert(pipelineError instanceof DOMException);
assertEquals(pipelineError.name, "GPUPipelineError");
assertEquals(pipelineError.message, "bad");
assertEquals(pipelineError.reason, "internal");
assertEquals(new GPUPipelineError(undefined, { reason: "validation" }).message, "");
assertThrows(
  () => new GPUPipelineError("x", { reason: "nope" }),
  TypeError,
  "invalid GPUPipelineErrorReason nope",
);

// GPUUncapturedErrorEvent is an Event carrying `error`, and the device
// dispatches one for an error no scope captured.
assertEquals(GPUUncapturedErrorEvent.length, 2);
const constructed = new GPUUncapturedErrorEvent("uncapturederror", {
  error: new GPUValidationError("oops"),
});
assert(constructed instanceof GPUUncapturedErrorEvent);
assert(constructed instanceof Event);
assertEquals(constructed.type, "uncapturederror");
assertEquals(constructed.error.message, "oops");
assertThrows(
  () => new GPUUncapturedErrorEvent("uncapturederror", {}),
  TypeError,
  "required member error is undefined",
);
const uncaptured = new Promise((resolve) => {
  device.addEventListener("uncapturederror", resolve);
});
device.createShaderModule({ code: "this is not WGSL" });
const delivered = await uncaptured;
assert(delivered instanceof GPUUncapturedErrorEvent);
assert(delivered.error instanceof GPUValidationError);

// An error scope still wins over the uncaptured path, and a failing call
// inside a scope still reports its own error first.
device.pushErrorScope("validation");
device.createShaderModule({ code: "this is not WGSL either" });
assert((await device.popErrorScope()) instanceof GPUValidationError);
device.destroy();
