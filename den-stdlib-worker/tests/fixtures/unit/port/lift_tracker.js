(function () {
      const port = new MessageChannel().port1;
      const handle = Object.getOwnPropertySymbols(port)
        .find((symbol) => symbol.description === "den:port-handle");
      if (handle === undefined) throw new TypeError("a MessagePort has no native handle");
      globalThis.nativeOf = (wrapper) => wrapper[handle];
      port.close();
    })
