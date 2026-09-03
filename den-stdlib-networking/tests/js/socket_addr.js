import { assert, assertEquals, assertThrows } from "den:assert";
import { UdpSocket } from "den:networking";

// SocketAddr is a read-only snapshot: the accessors report the bound address
// and, with no setters, an assignment is the TypeError strict code expects
// rather than a silently discarded write.
const socket = await UdpSocket.bind("127.0.0.1:0");
const addr = socket.localAddr;
assertEquals(typeof addr.port, "number");
assert(addr.port > 0);
assert(addr.is_ipv4);
assertEquals(addr.is_ipv6, false);
assert(addr.ip.is_loopback);
assertEquals(addr.toString(), `127.0.0.1:${addr.port}`);

const prototype = Object.getPrototypeOf(addr);
assertEquals(Object.getOwnPropertyDescriptor(prototype, "port").set, undefined);
assertEquals(Object.getOwnPropertyDescriptor(prototype, "ip").set, undefined);
const before = addr.port;
assertThrows(() => { addr.port = 1; }, TypeError);
assertThrows(() => { addr.ip = addr.ip; }, TypeError);
assertEquals(addr.port, before);
