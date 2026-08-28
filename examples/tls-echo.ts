// cargo run -- examples/tls-echo.ts
//
// Same shape as tcp-echo.ts, but the listener is TlsListener (PKCS#8
// identity) and the client is TlsStream.connect(addr, domain, caPem).
// The third argument is the CA the client should trust — required here
// because the cert is self-signed for CN=localhost.

import { TlsListener, TlsStream } from "den:networking";
import { connections, dest, writeAll } from "./net.ts";

const CERT: string = `-----BEGIN CERTIFICATE-----
MIIDCTCCAfGgAwIBAgIUc/NDTTAMGhjzYDVI0C7vPLEfiNowDQYJKoZIhvcNAQEL
BQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDgyNzA1MTIxNVoXDTM2MDgy
NDA1MTIxNVowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF
AAOCAQ8AMIIBCgKCAQEAw+roPpC/tWnKTnGUreFGfaRpXG6OoUYofG6wZJG2Tm2r
oFI0BoBC6uUOH6T6WHYk5vpoBbX712+X7Z6qmYCjdnjLtqCzOcylWvC9pG94QKc0
kEzDJZJkP4DXiVk6gIXPLQVKsYifONOfpoS+qhG935NbLk1DaEJPtCBhOmDr6EN4
sKObP2nNHN1c72fIflGsPZapuYS8ye0Q+eaC8XlPkuRbIbyUAH2HLAjGOgwG8zjq
TwKDdiKCn9fpZ/oMEMVo5yesznATS/uorE15tfHSkaPk7Lijb3WdPBEhoZR9ekxX
rQ95+S/OcVQCUaScgrLvuJFlHRw0GyUOdsScuOgEwQIDAQABo1MwUTAdBgNVHQ4E
FgQUR5Xts5h3ULANlazLlGeAlhvb8AIwHwYDVR0jBBgwFoAUR5Xts5h3ULANlazL
lGeAlhvb8AIwDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAOUlN
SSZUbq0NeZq4FrnObbsiAwBad0KTBw2BnbqKqhb82bnF6nTjvJnUivMQ1gmq5VXC
xk2pyH09SKqwdjyV2dRVKL/A0Cd+SNw5Sl9gFb3mwa5GVSCSjCMykEmHmVcOlayB
9RdhQ7PASLKQ//GJfVkkAtchlTA/Drb77XQyNLqZNGGEw+r8cOWuGcBBcY5HYOHk
CH4/KUMqch47CEdwJDE40TJxFXx3MEV0NtqAhGeyXh1DprQfC4heQFpkdcKonOF+
T+R0vQ+/vK0yDIlouJAV20QZkX4OZMeafEGuvRY6BT6kGQov0g2OPbBCfF74o7OL
0ms2nAVqqDsGZG598w==
-----END CERTIFICATE-----
`;

const KEY: string = `-----BEGIN PRIVATE KEY-----
MIIEuwIBADANBgkqhkiG9w0BAQEFAASCBKUwggShAgEAAoIBAQDD6ug+kL+1acpO
cZSt4UZ9pGlcbo6hRih8brBkkbZObaugUjQGgELq5Q4fpPpYdiTm+mgFtfvXb5ft
nqqZgKN2eMu2oLM5zKVa8L2kb3hApzSQTMMlkmQ/gNeJWTqAhc8tBUqxiJ8405+m
hL6qEb3fk1suTUNoQk+0IGE6YOvoQ3iwo5s/ac0c3VzvZ8h+Uaw9lqm5hLzJ7RD5
5oLxeU+S5FshvJQAfYcsCMY6DAbzOOpPAoN2IoKf1+ln+gwQxWjnJ6zOcBNL+6is
TXm18dKRo+TsuKNvdZ08ESGhlH16TFetD3n5L85xVAJRpJyCsu+4kWUdHDQbJQ52
xJy46ATBAgMBAAECgf9SJTKCI2Fkpy8qFZ/YksR6myqEB98UcTwHeW/XB7Zqz0Bt
axxNCFgYoiY7v7JCHoXLZTblvzvI9IRadBhqgMvs2Jz6gyBB9YxxlvwpbQh9soe2
UewiHVeKYwRlL6G0nnZn0NpGLyLgH67pKP/7CHdZ0AJXDvAKIa9590wHethpDryR
hru1n1l3KfGu5WmVv7DY4zKyrzOMKbgNnZ0iE1h8qMpmQZge+rquqmozJX9urlcm
Cs175/ISI+NnO7qnEjUYoGU5+HRy/7Z2HJ+XW7hFUn+juCw2hMX4/yeN1E4haG26
assMi2FbQ18NfVw5cmsPDCdLYi3fChbqUpmJZwECgYEA5LPABAfD7CGf4BgVVti2
v5bbzDPfXiGdNuOgpvlE/I1+VgT6rV5ZJni7FjzrZc20a0frNG2CIC/yWB3dLrTS
cfRak0SPp41VmrG05vlVF7Cyq6wzfNWaoHNS4/TAWMhVgxHr+d4OK5Y/0tDL6P7N
VuQoRx8UMOUUhCflSkFtz6MCgYEA201jh0ZZUztUmtIpyK/xkmdNwxfES5aF2HHZ
shvMuO2mRzT3BiWU3cn3nYyPBamsXLE4g558TIU2mpqOG5DDd60YCjorDxAaBnCc
VVxHw0ZrXrH9otssd1SsAN5jbW30G3lPLVxCrZIlJ20Quv7He3Re06yoGoJ7c2R/
6lBWEEsCgYA+x3DgKlmHyjsewr2o11hjA0BWr66TIlsLpDSHYUmkohqZ9kfxq0KB
owaINjTP/0WVZWqVO7JKr56wvZHnrk9OZKswXdOpRMzI6BsmhC7tj92b7ms7y07k
2INae+cI+AUxM4w5TNFK+bWPYy12SeuH/J1p2IgsW9Xj6Sex2IASTQKBgQC/V1V4
qPO1ADZAYxBb7s9qesHJb8owXWPoxuU3VrQXwhprVJYXgeDSZq6qgwIi4bjmoyX5
COXQ6gYLfMBy4qr5l0g7XCdHnDfo2IY+oCZpBd8Wn1v6pRq1/2WX2HGN//qVohFo
NXBj+vh53tpTHYs1dwJp0+JURvapZs2IxpFg4wKBgA/wjqq/OHC5j9WRSWj8kBK3
CmDs0BTA0SVsC5F5erfVlfFIcZmA0t50eXMXdQDCndAgwwD1jvHt+Pf7vq1tCTIg
R6zokU39/eIr4fS9FWnD8MzSStdO9IAyB4pDm8uMFIzmdZbSBC9zLjB7W/XandzJ
Qm+DmKXYXDY8HKm8PwpP
-----END PRIVATE KEY-----
`;

const listener = await TlsListener.listen("127.0.0.1:0", CERT, KEY);
const address: string = dest(listener);
console.log("listening tls", address);

const server = (async (): Promise<void> => {
  for await (const { stream, peer } of connections(listener)) {
    const chunk: Uint8Array = await stream.read(64);
    const text: string = new TextDecoder().decode(chunk);
    console.log("server received", JSON.stringify(text), "from", peer.toString());
    await writeAll(stream, "pong");
    await stream.shutdown();
    break;
  }
})();

const client = (async (): Promise<void> => {
  const stream = await TlsStream.connect(address, "localhost", CERT);
  await writeAll(stream, "ping");
  const reply: string = new TextDecoder().decode(await stream.read(64));
  console.log("client received", JSON.stringify(reply));
  await stream.shutdown();
})();

await Promise.all([server, client]);
