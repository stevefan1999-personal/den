export function firstMessage(worker) {
  return new Promise((resolve, reject) => {
    worker.onmessage = (event) => resolve(event.data);
    worker.onerror = (event) => reject(new Error(event.message));
  });
}
