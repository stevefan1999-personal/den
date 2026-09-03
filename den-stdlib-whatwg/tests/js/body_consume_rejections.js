import { assertEquals } from "den:assert";

// A consumer that cannot proceed rejects (never throws) with the TypeError the
// spec names.
const rejection = async (promise) => {
  assertEquals(promise instanceof Promise, true);
  try {
    await promise;
  } catch (error) {
    return `${error.constructor.name}:${error.message}`;
  }
  return "resolved";
};
const request = new Request("http://127.0.0.1/", { method: "POST", body: "x" });
await request.text();
assertEquals(await rejection(request.text()), "TypeError:Already read");
assertEquals(
  await rejection(new Request("http://127.0.0.1/").formData()),
  "TypeError:Failed to parse body as FormData",
);
const response = new Response("x");
await response.text();
assertEquals(await rejection(response.text()), "TypeError:Already read");
