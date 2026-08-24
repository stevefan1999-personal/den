import { assertEquals } from "den:assert";
import * as ns from "den:networking";

assertEquals(
  Object.keys(ns).sort().join(","),
  "TcpListener,TcpStream,TlsListener,TlsStream,UdpSocket,UnixListener,UnixStream,WebSocket",
);
