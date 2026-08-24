import { assert, assertEquals } from "den:assert";
const file = new File(["data"], "name.txt", { type: "text/plain", lastModified: 1 });
assertEquals(file.name, "name.txt");
assertEquals(file.type, "text/plain");
assertEquals(file.lastModified, 1);
assert(file instanceof Blob);
assert(file instanceof File);
assertEquals(file.size, 4);
