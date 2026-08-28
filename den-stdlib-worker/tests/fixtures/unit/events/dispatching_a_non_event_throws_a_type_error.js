try {
  new EventTarget().dispatchEvent({ type: "ping" });
  "no error"
} catch (error) { error.name }
