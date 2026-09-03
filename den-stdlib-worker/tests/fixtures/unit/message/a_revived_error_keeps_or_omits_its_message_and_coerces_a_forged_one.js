// The clone tag is an ordinary own key, so a script can forge one; the
// receiver must coerce whatever `message` it finds, never throw on it.
const tag = "\0den:structured-clone";
[
  structuredClone(new Error()).message,
  structuredClone(new Error("boom")).message,
  structuredClone({ [tag]: "Error", name: "Error", message: 42 }).message,
].join("|")
