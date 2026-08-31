const moved = body.transfer(2);
const movedView = view.buffer.transfer();
[new Uint8Array(moved).join("-"),
 String.fromCharCode(...new Uint8Array(movedView)),
 body.detached, view.byteLength].join(",")
