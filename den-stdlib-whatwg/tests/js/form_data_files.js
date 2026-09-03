import { assert, assertEquals } from "den:assert";

// Blob entries become Files named "blob" unless a filename is given; File
// entries keep their own name and lastModified unless renamed; anything else
// is stringified.
const blob = new Blob(["x"], { type: "text/plain" });
const file = new File(["y"], "own.txt", { type: "text/html", lastModified: 5 });
const form = new FormData();
form.append("blob", blob);
form.append("named", blob, "given.txt");
form.append("file", file);
form.append("renamed", file, "other.txt");
form.append("text", 12);

const asBlob = form.get("blob");
assert(asBlob instanceof File);
assertEquals([asBlob.name, asBlob.type, await asBlob.text()], ["blob", "text/plain", "x"]);
assertEquals(form.get("named").name, "given.txt");
const kept = form.get("file");
assertEquals(
  [kept.name, kept.type, kept.lastModified, await kept.text()],
  ["own.txt", "text/html", 5, "y"],
);
const renamed = form.get("renamed");
assertEquals([renamed.name, renamed.type, await renamed.text()], ["other.txt", "text/html", "y"]);
assertEquals(form.get("text"), "12");
