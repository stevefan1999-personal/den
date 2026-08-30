// cargo run -- examples/tls-echo.ts
//
// Same shape as tcp-echo.ts, but the listener is TlsListener (PKCS#8
// identity) and the client is TlsStream.connect(addr, domain, caPem).
// The third argument is the CA the client should trust — required here
// because the cert is self-signed for CN=localhost.

import { TlsListener, TlsStream } from "den:networking";
import { connections, dest, writeAll } from "./net.ts";

const CERT: string = `-----BEGIN CERTIFICATE-----
MIIDHDCCAgSgAwIBAgIUO+Ybd3SXlnu9WKo3uNxn+SpmzPswDQYJKoZIhvcNAQEL
BQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDgzMDExMjgyOVoXDTM2MDgy
NzExMjgyOVowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF
AAOCAQ8AMIIBCgKCAQEA0h04sw0Pl+K+YW5V45UUAohURBytsow68J6AatchxTc3
eJm62j09ms/XAaG4Z3RU++AJiUSc0hs2BmExFMdvj7j8nRMAMW0KmgCrs5o5YGW3
CSpuAQfpgaJvU/xYizu7gGL/5g2BoOhQpP9fqe34p3gKaMlS7iLaqV5p08fb1Gse
bkY/o4NVilfFREfikNrz1jMpklnAREbsG3WkpZv9acUG6hTBP1RT04GaLYE+Jhlh
8wdHGEru7lf4BdMKRygphIEkZniONnlaV4y8HY2vFlGX6D2sCkFsj6iEovzE2hAm
ydjsDtAphytI5Nfjj3dMN701cnQS43pzBKRUwIGplQIDAQABo2YwZDAdBgNVHQ4E
FgQUZ9hC5usOCHKoq9YEJYsijJ0qmmUwHwYDVR0jBBgwFoAUZ9hC5usOCHKoq9YE
JYsijJ0qmmUwFAYDVR0RBA0wC4IJbG9jYWxob3N0MAwGA1UdEwEB/wQCMAAwDQYJ
KoZIhvcNAQELBQADggEBALJsG0CUcTlDNfUXrg+l0ithYNwoTfVs9mpx0m1Y5DS9
LSJXVRO1jPa0T+2C5pYoaNy0AR8420yUzpLfUWPURA+WUeJ0w/SuNFMRzoJ5j3uB
rjFOxbaCpaQLQwm1Bc4PN3kG+snnPJsHYh2Yg8XvusBs9a6S2DAu9LWYuY2rjZcG
7yplSPqwS2QiOojGhdVETA+2rVpbXTbqWYm/pvvtJBI0VQVlaUyl0vxYZjXcqAPd
dbWE72Mfbq0R+Cf4OHGcKBujt/IKIyFWiZg9UZeCcTSmFmAobT95dzsBlQBIULkE
hgWrt6Nwa+LjETWkofXan4P9tMaK8qD61Giuk10Kv20=
-----END CERTIFICATE-----
`;

const KEY: string = `-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDSHTizDQ+X4r5h
blXjlRQCiFREHK2yjDrwnoBq1yHFNzd4mbraPT2az9cBobhndFT74AmJRJzSGzYG
YTEUx2+PuPydEwAxbQqaAKuzmjlgZbcJKm4BB+mBom9T/FiLO7uAYv/mDYGg6FCk
/1+p7fineApoyVLuItqpXmnTx9vUax5uRj+jg1WKV8VER+KQ2vPWMymSWcBERuwb
daSlm/1pxQbqFME/VFPTgZotgT4mGWHzB0cYSu7uV/gF0wpHKCmEgSRmeI42eVpX
jLwdja8WUZfoPawKQWyPqISi/MTaECbJ2OwO0CmHK0jk1+OPd0w3vTVydBLjenME
pFTAgamVAgMBAAECggEAJW+k7EwBsP1qj8f9oBqt8cSSBP+6GAOhtcD73u1dPDr6
FHAJiXxbV1O8OnNyvGYPBUCV2mIB8fJ8tfbHrKzbDPe39JGP2X6U4rsHXKz4F5uP
2N95MZBUE6+aF9Pwf8BBCF87OmVCKSXzRm6kwA1hHg+GhUSHlNvba01h3CSyH36y
iBWiShNzN0EyqMqI5lmKCVsrJ7eUXmY0Ejif20SrtsEuAL+ppfhacCVbxDX1bDRN
qTT88xMU0idO59F0Tb8fBnlDiLMBHTrgAtggdHczb4b+3/+A6nkCtYJtEM2sMdjw
qd+v+SOom3pAM5hoQVgBZVcUMPQgpG3o8ZCPaPH0aQKBgQD7T82O5Wd2i3y82fj/
Q7jUq5tqK9XKkdSEfFnu5SoIZkbOOAIwf380uoll3f29CIcoIlX+4sp0Vsa58TCi
krM6RdDtu7rIbApJbKIf452IYHbnG3UOEVRnCnZDzAS0CDgociW2kA/ececso7s3
4+3SDf2lHX8nEoIFWmiQCWetDQKBgQDWCKuDRfdRSS32QcyqqdWatFSpBi06FWu5
Xze6HfBl7GSgCRciIoR4jXYSSH479Vz/Xw7AT/EBcN0EBfhqV6NY1A4Y1x1MlxrQ
DQsguLAQdqvKw3JdZWEronNnbR7hzEcPT2641u6DstYXnu1wVT6JOQNsT2cqH29Y
sJjH+e8cqQKBgQCudClnxsvZuN6wYke9O4+04iOSwjc41Z7HEWOEuMRC7Gy+fpbW
f8sYGV2Dv2SCssbQD3XO6DROKmbtcQan9FpCW3C7dxQkSQujCKxKosEaiIxBxget
6k3C8bpDOf8R0prZSNPxNXQuoLcvf8FY/Pp8VIX89srrnqdve+EWC9FSiQKBgQCI
XzvQx5qeKy9i0WfzcYTNLosmqu3ULWPW18ltB7htaKJwqXoY4L9hBFkvqwrrbxmT
COEgPY9EqMHZ12gBcdd9OJfG0gE0FK8b0sO9VI+x3br11XQf+AFiyP4Y7xkXK443
PhhBI4kTVrY8lKGaymWvDymUMD9+Qksyykp+WEw3CQKBgEhW6y4tPNwrBZZKFc2Q
R0i5tVLkSWJjK48zGaUPiRZH622AHbuQ4vGdFHkYMUJB8CmunHlD+v2pMWOz+Dbs
Vavl/gpJocGAE1JW9+nTN0ooSeyhKKyof5mdd8LcnMH/mLHPNkKIG6p8kTfR3T2q
dpssUKg9z6uLbdstN7fYCApV
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
