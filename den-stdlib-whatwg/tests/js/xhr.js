import { assert, assertEquals } from "den:assert";

const getUrl = process.env.DEN_TEST_GET_URL;
const postUrl = process.env.DEN_TEST_POST_URL;
const get = await new Promise((resolve, reject) => {
  const xhr = new XMLHttpRequest();
  xhr.open("GET", getUrl);
  xhr.onload = () => resolve(xhr);
  xhr.onerror = () => reject(new Error("xhr error"));
  xhr.send();
});
const posted = await new Promise((resolve, reject) => {
  const xhr = new XMLHttpRequest();
  xhr.open("POST", postUrl);
  xhr.setRequestHeader("Content-Type", "text/plain");
  xhr.onload = () => resolve(xhr);
  xhr.onerror = () => reject(new Error("xhr error"));
  xhr.send("ping");
});
let syncThrew = false;
try {
  const xhr = new XMLHttpRequest();
  xhr.open("GET", getUrl, false);
} catch (error) {
  syncThrew = error instanceof TypeError;
}
assertEquals(get.status, 200);
assertEquals(get.responseText, "hello-xhr");
assertEquals(get.getResponseHeader("x-echo"), "yes");
assertEquals(get.readyState, XMLHttpRequest.DONE);
assertEquals(posted.responseText, "ping");
assertEquals(get.responseXML, null);
assert(get instanceof EventTarget);
assert(syncThrew);
