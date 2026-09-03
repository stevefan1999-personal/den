import { assertEquals } from "den:assert";

// Path segments percent-encode controls, DEL, space and " # < > ? ^ ` { };
// everything else, `%` included, passes through untouched.
const url = new URL("http://h/");
url.pathname = "a b\"c#d<e>f?g^h`i{j}k%20l";
assertEquals(url.pathname, "/a%20b%22c%23d%3Ce%3Ef%3Fg%5Eh%60i%7Bj%7Dk%20l");
url.pathname = "é\x7f\x01";
assertEquals(url.pathname, "/%C3%A9%7F%01");
assertEquals(new URL("file:///C:/a b/{c}").pathname, "/C:/a%20b/%7Bc%7D");
