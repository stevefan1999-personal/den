// den testharnessreport.js: record results on globalThis.__denWpt for the Rust walker.
// Must not create `document` — testharness.js would then pick WindowTestEnvironment
// and wait for a DOM `load` event that never arrives.

if (typeof globalThis.location === "undefined" || !globalThis.location.protocol) {
  globalThis.location = {
    href: "http://127.0.0.1/",
    protocol: "http:",
    host: "127.0.0.1",
    hostname: "127.0.0.1",
    port: "",
    pathname: "/",
    search: "",
    hash: "",
    origin: "http://127.0.0.1",
  };
}

if (typeof globalThis.escape !== "function") {
  globalThis.escape = function (value) {
    return encodeURIComponent(String(value)).replace(/[!'()*]/g, function (ch) {
      return "%" + ch.charCodeAt(0).toString(16).toUpperCase();
    });
  };
}

if (typeof globalThis.GLOBAL === "undefined") {
  globalThis.GLOBAL = {
    isWindow: function () {
      return typeof globalThis.document !== "undefined";
    },
    isWorker: function () {
      return typeof globalThis.document === "undefined";
    },
    isShadowRealm: function () {
      return false;
    },
  };
}

if (typeof globalThis.URL !== "function") {
  function denResolveUrl(base, ref) {
    ref = String(ref);
    if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(ref)) {
      return ref;
    }
    var parsed = denParseUrl(String(base));
    if (ref.slice(0, 2) === "//") {
      return parsed.protocol + ref;
    }
    if (ref.charAt(0) === "/") {
      return parsed.protocol + "//" + parsed.host + ref;
    }
    if (ref.charAt(0) === "?") {
      return parsed.protocol + "//" + parsed.host + parsed.pathname + ref;
    }
    if (ref.charAt(0) === "#") {
      return parsed.protocol + "//" + parsed.host + parsed.pathname + parsed.search + ref;
    }
    var dir = parsed.pathname.replace(/\/[^/]*$/, "/");
    var joined = dir + ref;
    var parts = joined.split("/");
    var out = [];
    for (var i = 0; i < parts.length; i++) {
      if (parts[i] === "..") {
        out.pop();
      } else if (parts[i] !== ".") {
        out.push(parts[i]);
      }
    }
    return parsed.protocol + "//" + parsed.host + out.join("/");
  }
  function denParseUrl(href) {
    var hash = "";
    var hashAt = href.indexOf("#");
    if (hashAt !== -1) {
      hash = href.slice(hashAt);
      href = href.slice(0, hashAt);
    }
    var search = "";
    var searchAt = href.indexOf("?");
    if (searchAt !== -1) {
      search = href.slice(searchAt);
      href = href.slice(0, searchAt);
    }
    var protoAt = href.indexOf("://");
    var protocol = href.slice(0, protoAt + 1);
    var rest = href.slice(protoAt + 3);
    var slash = rest.indexOf("/");
    var host = slash === -1 ? rest : rest.slice(0, slash);
    var pathname = slash === -1 ? "/" : rest.slice(slash);
    var colon = host.lastIndexOf(":");
    var hostname = host;
    var port = "";
    if (colon !== -1 && host.indexOf("]") === -1) {
      hostname = host.slice(0, colon);
      port = host.slice(colon + 1);
    }
    return {
      href: protocol + "//" + host + pathname + search + hash,
      protocol: protocol,
      host: host,
      hostname: hostname,
      port: port,
      pathname: pathname,
      search: search,
      hash: hash,
      origin: protocol + "//" + host,
    };
  }
  globalThis.URL = function URL(url, base) {
    var href = base == null ? String(url) : denResolveUrl(base.href || base, url);
    if (!/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(href)) {
      throw new TypeError("Invalid URL");
    }
    var parsed = denParseUrl(href);
    this._protocol = parsed.protocol;
    this._hostname = parsed.hostname;
    this._port = parsed.port;
    this._pathname = parsed.pathname;
    this._search = parsed.search;
    this._hash = parsed.hash;
    this._rebuild();
  };
  globalThis.URL.prototype._rebuild = function () {
    this._host = this._hostname + (this._port ? ":" + this._port : "");
    this.origin = this._protocol + "//" + this._host;
    this.href = this._protocol + "//" + this._host + this._pathname + this._search + this._hash;
  };
  function denUrlProp(key, extra) {
    Object.defineProperty(globalThis.URL.prototype, key, {
      get: function () {
        return this["_" + key];
      },
      set: function (value) {
        this["_" + key] = String(value);
        if (extra) extra.call(this);
        this._rebuild();
      },
    });
  }
  denUrlProp("protocol");
  denUrlProp("hostname");
  denUrlProp("port");
  denUrlProp("pathname");
  denUrlProp("search");
  denUrlProp("hash");
  Object.defineProperty(globalThis.URL.prototype, "host", {
    get: function () {
      return this._host;
    },
    set: function (value) {
      value = String(value);
      var colon = value.lastIndexOf(":");
      if (colon !== -1 && value.indexOf("]") === -1) {
        this._hostname = value.slice(0, colon);
        this._port = value.slice(colon + 1);
      } else {
        this._hostname = value;
        this._port = "";
      }
      this._rebuild();
    },
  });
  globalThis.URL.prototype.toString = function () {
    return this.href;
  };
  globalThis.URL.prototype.toJSON = function () {
    return this.href;
  };
  Object.defineProperty(globalThis.URL.prototype, "searchParams", {
    get: function () {
      if (!this._searchParams) {
        var params = new globalThis.URLSearchParams(this.search);
        var url = this;
        var sync = function () {
          var query = params.toString();
          url.search = query ? "?" + query : "";
          url.href = url.protocol + "//" + url.host + url.pathname + url.search + url.hash;
        };
        var wrap = function (name) {
          var inner = params[name];
          params[name] = function () {
            var result = inner.apply(params, arguments);
            sync();
            return result;
          };
        };
        wrap("append");
        wrap("set");
        wrap("delete");
        this._searchParams = params;
      }
      return this._searchParams;
    },
  });
}

if (typeof globalThis.URLSearchParams !== "function") {
  globalThis.URLSearchParams = function URLSearchParams(search) {
    this._pairs = [];
    var query = String(search == null ? "" : search);
    if (query.charAt(0) === "?") {
      query = query.slice(1);
    }
    if (!query) {
      return;
    }
    query.split("&").forEach(function (part) {
      if (!part) {
        return;
      }
      var eq = part.indexOf("=");
      var key = decodeURIComponent((eq === -1 ? part : part.slice(0, eq)).replace(/\+/g, " "));
      var val = decodeURIComponent((eq === -1 ? "" : part.slice(eq + 1)).replace(/\+/g, " "));
      this._pairs.push([key, val]);
    }, this);
  };
  globalThis.URLSearchParams.prototype.get = function (name) {
    for (var index = 0; index < this._pairs.length; index++) {
      if (this._pairs[index][0] === name) {
        return this._pairs[index][1];
      }
    }
    return null;
  };
  globalThis.URLSearchParams.prototype.getAll = function (name) {
    var found = [];
    for (var index = 0; index < this._pairs.length; index++) {
      if (this._pairs[index][0] === name) {
        found.push(this._pairs[index][1]);
      }
    }
    return found;
  };
  globalThis.URLSearchParams.prototype.append = function (name, value) {
    this._pairs.push([String(name), String(value)]);
  };
  globalThis.URLSearchParams.prototype.set = function (name, value) {
    name = String(name);
    value = String(value);
    var replaced = false;
    this._pairs = this._pairs.filter(function (pair) {
      if (pair[0] === name) {
        if (replaced) {
          return false;
        }
        pair[1] = value;
        replaced = true;
      }
      return true;
    });
    if (!replaced) {
      this._pairs.push([name, value]);
    }
  };
  globalThis.URLSearchParams.prototype.delete = function (name) {
    name = String(name);
    this._pairs = this._pairs.filter(function (pair) {
      return pair[0] !== name;
    });
  };
  globalThis.URLSearchParams.prototype.toString = function () {
    return this._pairs
      .map(function (pair) {
        return encodeURIComponent(pair[0]) + "=" + encodeURIComponent(pair[1]);
      })
      .join("&");
  };
}

if (typeof XMLHttpRequest === "function" && XMLHttpRequest.prototype.setRequestHeader) {
  var denXhrSet = XMLHttpRequest.prototype.setRequestHeader;
  XMLHttpRequest.prototype.setRequestHeader = function (name, value) {
    if (String(name).indexOf("\0") !== -1 || String(value).indexOf("\0") !== -1) {
      throw new DOMException("The string contains invalid characters.", "SyntaxError");
    }
    return denXhrSet.call(this, name, value);
  };
}

if (typeof ReadableStream === "function" && typeof ReadableStream.prototype.pipeTo !== "function") {
  ReadableStream.prototype.pipeTo = function (dest) {
    var reader = this.getReader();
    var write = dest && typeof dest.write === "function" ? dest.write.bind(dest) : null;
    function pump() {
      return reader.read().then(function (result) {
        if (result.done) {
          if (dest && typeof dest.close === "function") {
            return dest.close();
          }
          return;
        }
        if (write) {
          return Promise.resolve(write(result.value)).then(pump);
        }
        return pump();
      });
    }
    return pump();
  };
}

// testharness has already selected its environment. Install only
// the tiny DOM surface used by the runnable FileAPI constructor cases.
function denTagged(value, name) {
    Object.defineProperty(value, Symbol.toStringTag, { value: name, configurable: true });
    return value;
  }

  function denCollection(items, name) {
    var collection = denTagged({}, name);
    for (var index = 0; index < items.length; index++) {
      collection[index] = items[index];
    }
    Object.defineProperty(collection, "length", {
      configurable: true,
      get: function () {
        return items.length;
      },
      set: function (length) {
        items.length = Math.max(0, Number(length) || 0);
      },
    });
    collection[Symbol.iterator] = function () {
      return items[Symbol.iterator]();
    };
    return collection;
  }

  function denElement(name) {
    name = String(name || "").toLowerCase();
    var type = {
      body: "HTMLBodyElement",
      div: "HTMLDivElement",
      html: "HTMLHtmlElement",
      option: "HTMLOptionElement",
      p: "HTMLParagraphElement",
      select: "HTMLSelectElement",
    }[name] || "HTMLElement";
    var element = denTagged({}, type);
    var children = [];
    var attributes = [];
    element.tagName = name.toUpperCase();
    element.localName = name;
    element.namespaceURI = "http://www.w3.org/1999/xhtml";
    element.children = denCollection(children, "HTMLCollection");
    element.attributes = denCollection(attributes, "NamedNodeMap");
    element.appendChild = function (child) {
      children.push(child);
      element.children = denCollection(children, "HTMLCollection");
      if (name === "select") {
        element[children.length - 1] = child;
        element.length = children.length;
        element[Symbol.iterator] = function () {
          return children[Symbol.iterator]();
        };
      }
      return child;
    };
    element.setAttribute = function (attributeName, value) {
      var attribute = denTagged({ name: String(attributeName), value: String(value) }, "Attr");
      attributes.push(attribute);
      element.attributes = denCollection(attributes, "NamedNodeMap");
    };
    return element;
  }

  globalThis.document = denTagged({ readyState: "complete" }, "HTMLDocument");
  globalThis.document.body = denElement("body");
  globalThis.document.documentElement = denElement("html");
  globalThis.document.defaultView = globalThis;
  globalThis.document.createElement = denElement;
  globalThis.document.createElementNS = function (_, name) {
    return denElement(name);
  };
  globalThis.document.getElementsByTagName = function () {
    return denCollection([], "HTMLCollection");
  };
  globalThis.document.getElementById = function () {
    return null;
  };
  globalThis.parent = globalThis;
globalThis.top = globalThis;

setup({ output: false, explicit_timeout: true, explicit_done: true });

globalThis.__denWpt = { done: false, harness: 0, rows: [] };

add_completion_callback(function (tests, harness_status) {
  globalThis.__denWpt.harness = harness_status.status;
  globalThis.__denWpt.rows = tests.map(function (entry) {
    return {
      name: entry.name,
      status: entry.status,
      message: entry.message ? String(entry.message) : "",
    };
  });
  globalThis.__denWpt.done = true;
});

globalThis.__denWptEncode = function __denWptEncode() {
  var lines = ["HARNESS\t" + globalThis.__denWpt.harness];
  var rows = globalThis.__denWpt.rows;
  for (var index = 0; index < rows.length; index++) {
    var row = rows[index];
    lines.push(
      row.status +
        "\t" +
        String(row.name).replace(/\t/g, " ").replace(/\n/g, " ") +
        "\t" +
        String(row.message).replace(/\t/g, " ").replace(/\n/g, " "),
    );
  }
  return lines.join("\n");
};

globalThis.__denWptWait = async function __denWptWait(ms) {
  var deadline = Date.now() + ms;
  while (!globalThis.__denWpt.done) {
    if (Date.now() > deadline) {
      return "TIMEOUT\n" + globalThis.__denWptEncode();
    }
    await new Promise(function (resolve) {
      setTimeout(resolve, 15);
    });
  }
  return globalThis.__denWptEncode();
};
