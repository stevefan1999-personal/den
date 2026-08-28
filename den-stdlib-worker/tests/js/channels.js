import { assertEquals } from "den:assert";

const channel = new MessageChannel();
const direct = new Promise((resolve) => {
  channel.port1.onmessage = ({ data }) => resolve(data);
});
channel.port2.postMessage({ value: 42 });
assertEquals((await direct).value, 42);
channel.port1.close();
channel.port2.close();

const sender = new BroadcastChannel("fixture-channel");
const receiver = new BroadcastChannel("fixture-channel");
const broadcast = new Promise((resolve) => {
  receiver.onmessage = ({ data }) => resolve(data);
});
sender.postMessage("heard");
assertEquals(await broadcast, "heard");
sender.close();
receiver.close();
