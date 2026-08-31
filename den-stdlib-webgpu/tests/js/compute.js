import { assert, assertEquals } from "den:assert";
import {
  GPUBufferUsage,
  GPUMapMode,
  GPUValidationError,
  gpu,
} from "den:webgpu";

const adapter = await gpu.requestAdapter();
if (adapter !== null) {
  const device = await adapter.requestDevice();
  device.pushErrorScope("validation");
  device.createShaderModule({ code: "this is not WGSL" });
  const validation = await device.popErrorScope();
  assert(validation instanceof GPUValidationError);

  const storage = device.createBuffer({
    size: 16,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
    mappedAtCreation: true,
  });
  new Uint32Array(storage.getMappedRange()).set([0, 1, 2, 3]);
  storage.unmap();

  const readback = device.createBuffer({
    size: 16,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
  });
  const shader = device.createShaderModule({
    code: `
      @group(0) @binding(0) var<storage, read_write> data: array<u32>;

      @compute @workgroup_size(1)
      fn main(@builtin(global_invocation_id) id: vec3<u32>) {
        data[id.x] *= 2u;
      }
    `,
  });
  const pipeline = device.createComputePipeline({
    layout: "auto",
    compute: { module: shader, entryPoint: "main" },
  });
  const bindGroup = device.createBindGroup({
    layout: pipeline.getBindGroupLayout(0),
    entries: [{ binding: 0, resource: { buffer: storage } }],
  });
  const encoder = device.createCommandEncoder();
  const pass = encoder.beginComputePass();
  pass.setPipeline(pipeline);
  pass.setBindGroup(0, bindGroup);
  pass.dispatchWorkgroups(4);
  pass.end();
  encoder.copyBufferToBuffer(storage, 0, readback, 0, 16);
  device.queue.submit([encoder.finish()]);

  await readback.mapAsync(GPUMapMode.READ);
  assertEquals([...new Uint32Array(readback.getMappedRange())], [0, 2, 4, 6]);
  readback.unmap();
  storage.destroy();
  readback.destroy();
  device.destroy();
}
