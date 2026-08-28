import { assertEquals } from "den:assert";

const { default: ms } = await import("https://esm.sh/ms@2.1.3");

assertEquals(ms("1s"), 1000);
assertEquals(ms(2000), "2s");
