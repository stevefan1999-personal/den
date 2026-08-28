self.onmessage = async (event) => {
  try {
    const response = await fetch(event.data);
    postMessage({ ok: true, text: await response.text() });
  } catch (error) {
    postMessage({ ok: false, message: String(error) });
  }
};
