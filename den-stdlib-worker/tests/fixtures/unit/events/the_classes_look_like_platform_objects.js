[
  Object.prototype.toString.call(new EventTarget()),
  Object.prototype.toString.call(new MessageEvent("message")),
  Object.prototype.toString.call(new ErrorEvent("error")),
  Object.keys(EventTarget.prototype).length,
  EventTarget.prototype.addEventListener.length,
  EventTarget.prototype.dispatchEvent.length,
  Event.length, MessageEvent.name,
  Event.AT_TARGET, new Event("ping").eventPhase,
  new MessageEvent("message") instanceof Event,
].join(",")
