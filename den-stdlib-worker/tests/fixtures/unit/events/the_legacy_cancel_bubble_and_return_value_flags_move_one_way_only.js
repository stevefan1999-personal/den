const event = new Event("ping", { cancelable: true });
const fresh = `${event.cancelBubble},${event.returnValue}`;
event.cancelBubble = false;
event.returnValue = true;
const unchanged = `${event.cancelBubble},${event.returnValue}`;
event.cancelBubble = true;
event.returnValue = false;
const set = `${event.cancelBubble},${event.returnValue},${event.defaultPrevented}`;
const plain = new Event("ping");
plain.returnValue = false;
`${fresh}|${unchanged}|${set}|${plain.returnValue}`
