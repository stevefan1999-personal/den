// cargo run -- --config examples/import-map/den.json examples/import-map/main.ts
//
// den.json `imports` is an import map. Relative targets resolve against the
// config file's directory. Bare specifiers that the map does not name still
// resolve as files.

import { greet } from "greet";

console.log(greet("den"));
