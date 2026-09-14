/** In-process observer bridge; the DSH registry preserves the AbortSignal identity. */
const observers = new WeakMap()
export function observeProgress(signal, callback) {
  observers.set(signal, callback)
  return () => observers.delete(signal)
}
export function reportProgress(signal, event) {
  observers.get(signal)?.(event)
}
