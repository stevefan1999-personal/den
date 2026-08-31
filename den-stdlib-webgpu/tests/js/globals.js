import { assert, assertEquals } from "den:assert";

assertEquals(typeof GPU, "function");
assertEquals(typeof GPUBuffer, "function");
assertEquals(typeof GPUValidationError, "function");
assertEquals(GPUBufferUsage.COPY_DST, 8);
assertEquals(GPUMapMode.READ, 1);
assert(navigator.gpu instanceof GPU);
assertEquals(typeof navigator.gpu.requestAdapter, "function");

const installed = navigator.gpu;
const { gpu, GPUValidationError: exportedValidationError } = await import("den:webgpu");
assert(gpu === installed);
assert(gpu === navigator.gpu);
assert(exportedValidationError === GPUValidationError);
