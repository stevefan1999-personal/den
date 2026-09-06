// cargo run -- examples/udp-echo.ts
//
// Two UdpSockets on the same event loop: bind :0, sendTo, recvFrom.

import { UdpSocket } from "den:networking";

const receiver = await UdpSocket.bind("127.0.0.1:0");
const sender = await UdpSocket.bind("127.0.0.1:0");
const dest = `127.0.0.1:${receiver.localAddr.port}`;

const incoming = receiver.recvFrom(64);
const sent = await sender.sendTo(new TextEncoder().encode("ping"), dest);
const [payload, from] = await incoming;
console.log("sent", sent, "bytes");
console.log("recv", JSON.stringify(new TextDecoder().decode(payload)), "from", from.toString());
