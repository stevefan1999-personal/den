import { assertEquals } from "den:assert";
assertEquals(`${btoa("den runtime")}|${atob(btoa("den runtime"))}`, "ZGVuIHJ1bnRpbWU=|den runtime");
