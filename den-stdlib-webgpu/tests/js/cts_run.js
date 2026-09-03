if (typeof globalThis.window === "undefined") {
  globalThis.window = { location: new URL("http://localhost/cts") };
}
if (typeof globalThis.self === "undefined") {
  globalThis.self = globalThis;
}

const ctsRoot = process.argv[2];
const specPath = process.argv[3];
const filePathParts = JSON.parse(process.argv[4] ?? "[]");

const { Logger } = await import(`${ctsRoot}/common/internal/logging/logger.js`);
const { TestQuerySingleCase } = await import(
  `${ctsRoot}/common/internal/query/query.js`
);
const { globalTestConfig } = await import(
  `${ctsRoot}/common/framework/test_config.js`
);
globalTestConfig.maxSubcasesInFlight = 1;
globalTestConfig.subcasesBetweenAttemptingGC = Number.POSITIVE_INFINITY;
globalTestConfig.casesBetweenReplacingDevice = 16;
const runtimeGc = globalThis.gc;
try {
  globalThis.gc = undefined;
} catch {
  // gc may be non-writable
}
const { g } = await import(specPath);
if (g === undefined || typeof g.iterate !== "function") {
  throw new Error(`CTS spec ${specPath} did not export a test group as g`);
}

const failures = [];
let cases = 0;
for (const test of g.iterate()) {
  for (const kase of test.iterate(null)) {
    const name = `${filePathParts.join(",")}:${test.testPath.join(",")}:${JSON.stringify(kase.id.params)}`;
    if (kase.isUnimplemented) {
      continue;
    }
    // One logger per case so passing results are unreachable for GC. A single
    // Logger.results Map retains every createTexture cartesian case and OOMs.
    const log = new Logger();
    const [rec, result] = log.record(name);
    const query = new TestQuerySingleCase(
      "webgpu",
      filePathParts,
      test.testPath,
      kase.id.params,
    );
    if (cases % 100 === 0) {
      console.error(`cts ${cases} ${name}`);
    }
    await kase.run(rec, query, []);
    if (typeof runtimeGc === "function") {
      runtimeGc();
    }
    if (result.status === "fail" || result.status === "warn") {
      const logs = (result.logs ?? [])
        .map((entry) => String(entry))
        .filter((line) => !line.includes("INFO: subcase ran"))
        .join("\n");
      const clipped = logs.length > 4000 ? `…\n${logs.slice(-4000)}` : logs;
      failures.push(`${name} [${result.status}]\n${clipped}`);
      if (failures.length >= 8) {
        throw new Error(
          `${failures.length} CTS case(s) failed (stopping early)\n${failures.join("\n\n")}`,
        );
      }
    }
    cases += 1;
  }
}

if (failures.length > 0) {
  throw new Error(`${failures.length} CTS case(s) failed\n${failures.join("\n\n")}`);
}
