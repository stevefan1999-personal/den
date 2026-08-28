["transfer", "transferToFixedLength", "transferToImmutable", "resize"]
  .map((name) => {
    const descriptor = Object.getOwnPropertyDescriptor(buf, name);
    return `${name}: writable=${descriptor.writable}, configurable=${descriptor.configurable}, enumerable=${descriptor.enumerable}`;
  })
  .join("\n")
