import { assert, assertEquals } from "den:assert";
import {
  GPUBufferUsage,
  GPUMapMode,
  GPUShaderStage,
  GPUTextureUsage,
  GPUValidationError,
  gpu,
} from "den:webgpu";

assert(gpu === navigator.gpu);
assertEquals(GPUBufferUsage.COPY_DST, 8);
assert(gpu.requestAdapter() instanceof Promise);

const adapter = await gpu.requestAdapter();
assert(adapter !== null);
assert(adapter.features instanceof GPUSupportedFeatures);
assert(typeof adapter.features.has === "function");
assert(adapter.limits instanceof GPUSupportedLimits);
assert(typeof adapter.limits.maxBufferSize === "number");
assert(adapter.features === adapter.features);
assert(adapter.limits === adapter.limits);

const device = await adapter.requestDevice({ defaultQueue: { label: "default" } });
assert(device.queue === device.queue);
assertEquals(device.queue.label, "default");
assert(device.features.has("core-features-and-limits"));
assertEquals(Object.getPrototypeOf(device), GPUDevice.prototype);
{
  let addEventListenerWasCalled = false;
  let dispatchEventWasCalled = false;
  let removeEventListenerWasCalled = false;
  const origAdd = EventTarget.prototype.addEventListener;
  const origDispatch = EventTarget.prototype.dispatchEvent;
  const origRemove = EventTarget.prototype.removeEventListener;
  EventTarget.prototype.addEventListener = function (...args) {
    addEventListenerWasCalled = true;
    return origAdd.call(this, ...args);
  };
  EventTarget.prototype.dispatchEvent = function (event) {
    dispatchEventWasCalled = true;
    return origDispatch.call(this, event);
  };
  EventTarget.prototype.removeEventListener = function (...args) {
    removeEventListenerWasCalled = true;
    return origRemove.call(this, ...args);
  };
  await new Promise((resolve) => {
    device.addEventListener("foo", resolve);
    device.dispatchEvent(new Event("foo"));
    device.removeEventListener("foo", resolve);
  });
  EventTarget.prototype.addEventListener = origAdd;
  EventTarget.prototype.dispatchEvent = origDispatch;
  EventTarget.prototype.removeEventListener = origRemove;
  assert(addEventListenerWasCalled);
  assert(dispatchEventWasCalled);
  assert(removeEventListenerWasCalled);
}

device.pushErrorScope("validation");
device.createShaderModule({ code: "this is not WGSL" });
const validation = await device.popErrorScope();
assert(validation instanceof GPUValidationError);

const buffer = device.createBuffer({
  label: "bytes",
  size: 16,
  usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
});
assertEquals(buffer.label, "bytes");
assertEquals(buffer.size, 16);
assertEquals(buffer.mapState, "unmapped");
device.queue.writeBuffer(buffer, 0, new Uint32Array([1, 2, 3, 4]), 1, 2);
await buffer.mapAsync(GPUMapMode.READ);
assertEquals([...new Uint32Array(buffer.getMappedRange())], [2, 3, 0, 0]);
buffer.unmap();
buffer.destroy();

{
  device.pushErrorScope("validation");
  const invalidTex = device.createTexture({
    size: [0, 0, 0],
    format: "rgba8unorm",
    usage: GPUTextureUsage.TEXTURE_BINDING,
  });
  assert((await device.popErrorScope()) instanceof GPUValidationError);

  device.pushErrorScope("validation");
  const invalidView = invalidTex.createView();
  assert((await device.popErrorScope()) instanceof GPUValidationError);

  const sampledLayout = device.createBindGroupLayout({
    entries: [{
      binding: 0,
      visibility: GPUShaderStage.COMPUTE,
      texture: { sampleType: "float" },
    }],
  });
  device.pushErrorScope("validation");
  device.createBindGroup({
    layout: sampledLayout,
    entries: [{ binding: 0, resource: invalidView }],
  });
  assert((await device.popErrorScope()) instanceof GPUValidationError);

  const uniformLayout = device.createBindGroupLayout({
    entries: [{
      binding: 0,
      visibility: GPUShaderStage.COMPUTE,
      buffer: { type: "uniform" },
    }],
  });
  device.pushErrorScope("validation");
  const invalidBuf = device.createBuffer({
    size: 16,
    usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_SRC,
  });
  assert((await device.popErrorScope()) instanceof GPUValidationError);
  device.pushErrorScope("validation");
  device.createBindGroup({
    layout: uniformLayout,
    entries: [{ binding: 0, resource: { buffer: invalidBuf } }],
  });
  assert((await device.popErrorScope()) instanceof GPUValidationError);

  device.pushErrorScope("validation");
  const invalidSampler = device.createSampler({ lodMinClamp: -1 });
  assert((await device.popErrorScope()) instanceof GPUValidationError);
  const samplerLayout = device.createBindGroupLayout({
    entries: [{
      binding: 0,
      visibility: GPUShaderStage.COMPUTE,
      sampler: { type: "filtering" },
    }],
  });
  device.pushErrorScope("validation");
  device.createBindGroup({
    layout: samplerLayout,
    entries: [{ binding: 0, resource: invalidSampler }],
  });
  assert((await device.popErrorScope()) instanceof GPUValidationError);
}

{
  const mapBuf = device.createBuffer({
    size: 16,
    usage: GPUBufferUsage.MAP_WRITE,
  });
  let firstName;
  const first = mapBuf.mapAsync(GPUMapMode.WRITE).then(
    () => {
      throw new Error("first mapAsync should abort");
    },
    (error) => {
      firstName = error.name;
    },
  );
  device.pushErrorScope("validation");
  let secondName;
  const second = mapBuf.mapAsync(GPUMapMode.WRITE).then(
    () => {
      throw new Error("second mapAsync should reject");
    },
    (error) => {
      secondName = error.name;
    },
  );
  assert((await device.popErrorScope()) instanceof GPUValidationError);
  mapBuf.unmap();
  await first;
  await second;
  assertEquals(firstName, "AbortError");
  assertEquals(secondName, "OperationError");
  await mapBuf.mapAsync(GPUMapMode.WRITE);
  mapBuf.unmap();
  mapBuf.destroy();
}

device.destroy();
