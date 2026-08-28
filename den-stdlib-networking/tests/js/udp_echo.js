import { assert, assertEquals } from "den:assert";
import { UdpSocket } from "den:networking";

const receiver = await UdpSocket.bind("127.0.0.1:0");
const sender = await UdpSocket.bind("127.0.0.1:0");
const dest = `127.0.0.1:${receiver.localAddr.port}`;

const incoming = receiver.recvFrom(64);
const sent = await sender.sendTo(new TextEncoder().encode("ping"), dest);
assertEquals(sent, 4);

const result = await incoming;
const payload = result[0];
const from = result[1];
assertEquals(new TextDecoder().decode(payload), "ping");
assert(from.is_ipv4);
assertEquals(from.port, sender.localAddr.port);
assert(from.ip.is_loopback);
