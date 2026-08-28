import { assertEquals } from "den:assert";

assertEquals(btoa(123), btoa("123"));
assertEquals(atob(btoa(true)), "true");
