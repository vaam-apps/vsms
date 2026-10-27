// A one-line bus for "the gateway just refused this browser's key". The sealing
// fetch reports every 401 it produces or receives; the gate listens and
// re-reads the key's status, which decides whether that means "not paired yet",
// "expired" or "revoked". Kept out of React so the transport does not import it.

type Listener = () => void;
const listeners = new Set<Listener>();

/** Reports a 401. */
export function notifyUnauthenticated(): void {
  for (const listener of listeners) listener();
}

/** Listens for 401s; returns the unsubscribe. */
export function onUnauthenticated(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
