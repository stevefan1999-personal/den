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
const { g } = await import(specPath);
if (g === undefined || typeof g.iterate !== "function") {
  throw new Error(`CTS spec ${specPath} did not export a test group as g`);
}

const log = new Logger();
const failures = [];
for (const test of g.iterate()) {
  for (const kase of test.iterate(null)) {
    const name = `${filePathParts.join(",")}:${test.testPath.join(",")}:${JSON.stringify(kase.id.params)}`;
    if (kase.isUnimplemented) {
      continue;
    }
    const [rec, result] = log.record(name);
    const query = new TestQuerySingleCase(
      "webgpu",
      filePathParts,
      test.testPath,
      kase.id.params,
    );
    await kase.run(rec, query, []);
    if (result.status === "fail" || result.status === "warn") {
      const logs = (result.logs ?? []).map((entry) => String(entry)).join("\n");
      failures.push(`${name} [${result.status}]\n${logs}`);
    }
  }
}

if (failures.length > 0) {
  throw new Error(`${failures.length} CTS case(s) failed\n${failures.join("\n\n")}`);
}
