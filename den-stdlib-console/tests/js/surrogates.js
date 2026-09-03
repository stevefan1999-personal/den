import { assertEquals, assertThrows } from "den:assert";

// The String/Int/Bool arms of `format_value` all funnel through QuickJS's
// UTF-8 conversion; a lone surrogate is the one input that makes it fail, and
// it must stay a TypeError rather than becoming a silent replacement.
const throws = assertThrows(() => console.log(1, "\uD800"), TypeError);
assertEquals(String(throws).includes("invalid utf-8"), true);
assertThrows(() => console.log({ key: "\uD800" }), TypeError);

// Everything well-formed still renders the same way.
console.log(1, "ok", true, 42, 1.5, "😀");
assertEquals(typeof console.log, "function");
