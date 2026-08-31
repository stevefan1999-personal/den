import { assert, assertEquals } from "den:assert";
import {
  GPUBufferUsage,
  GPUMapMode,
  GPUValidationError,
  gpu,
} from "den:webgpu";

assert(gpu === navigator.gpu);
assertEquals(GPUBufferUsage.COPY_DST, 8);
assert(gpu.requestAdapter() instanceof Promise);

const adapter = await gpu.requestAdapter();
assert(adapter !== null);
assert(adapter.features instanceof Set);
assert(typeof adapter.limits.maxBufferSize === "number");

const device = await adapter.requestDevice({ defaultQueue: { label: "default" } });
assert(device.queue === device.queue);
assertEquals(device.queue.label, "default");

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
device.destroy();
