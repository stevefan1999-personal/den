import { assertEquals } from "den:assert";

// set() overwrites the first match in place, drops the later ones, and
// appends when there is none.
const params = new URLSearchParams("a=1&b=2&a=3&c=4");
params.set("a", "9");
assertEquals(params.toString(), "a=9&b=2&c=4");
params.set("d", "5");
assertEquals(params.toString(), "a=9&b=2&c=4&d=5");

const form = new FormData();
for (const [name, value] of [["a", "1"], ["b", "2"], ["a", "3"], ["c", "4"]]) {
  form.append(name, value);
}
form.set("a", "9");
assertEquals([...form].map((pair) => pair.join("=")).join("&"), "a=9&b=2&c=4");
form.set("d", "5");
assertEquals([...form].map((pair) => pair.join("=")).join("&"), "a=9&b=2&c=4&d=5");
