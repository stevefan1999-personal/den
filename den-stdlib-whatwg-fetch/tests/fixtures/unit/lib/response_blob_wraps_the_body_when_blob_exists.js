(async () => {
  return [
    blob instanceof Blob,
    blob.type,
    await blob.text(),
  ].join("|");
})()
