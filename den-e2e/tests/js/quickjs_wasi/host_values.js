import { assertEquals } from "den:assert";
import { loadQuickJS } from "./load.js";

const { vm } = await loadQuickJS();

const config = vm.newObject();
const name = vm.newString("den");
const port = vm.newNumber(8080);
config.setProp("name", name);
config.setProp("port", port);
vm.global.setProp("config", config);
name.dispose();
port.dispose();
config.dispose();

const joined = vm.evalCode("config.name + ':' + config.port");
assertEquals(joined.toString(), "den:8080");
joined.dispose();

const handle = vm.hostToHandle({ ok: true, values: [1, 2, 3] });
vm.global.setProp("payload", handle);
handle.dispose();

const dumped = vm.evalCode("payload");
assertEquals(vm.dump(dumped), { ok: true, values: [1, 2, 3] });
dumped.dispose();

const items = vm.newArray();
const first = vm.newNumber(40);
const second = vm.newNumber(2);
items.setProp("0", first);
items.setProp("1", second);
const length = vm.newNumber(2);
items.setProp("length", length);
vm.global.setProp("items", items);
first.dispose();
second.dispose();
length.dispose();
items.dispose();

const summed = vm.evalCode("items[0] + items[1]");
assertEquals(summed.toNumber(), 42);
summed.dispose();

const huge = vm.newBigInt(2n ** 60n);
vm.global.setProp("huge", huge);
huge.dispose();
const next = vm.evalCode("huge + 1n");
assertEquals(next.toString(), String(2n ** 60n + 1n));
next.dispose();

vm.dispose();
