import { assertEquals, assertThrows } from "den:assert";

assertEquals(atob("YQ=="), "a");
assertEquals(btoa(""), "");
assertThrows(() => atob("!!!"));
