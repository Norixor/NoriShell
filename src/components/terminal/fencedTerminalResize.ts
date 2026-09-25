export interface TerminalDimensions {
  rows: number;
  cols: number;
}

interface ResolvedResize {
  key: string;
  send(sequence: string): Promise<void>;
}

/**
  * Retain the latest terminal size and submit it only when the caller can resolve a current valid fence.
  * Failures never guess write results; the next fit or explicit view reassertion replays the latest size.
 */
export function createFencedTerminalResize(
  resolve: (dimensions: TerminalDimensions) => ResolvedResize | null,
) {
  let sequence = 0n;
  let pending: { dimensions: TerminalDimensions; force: boolean } | null = null;
  let appliedKey: string | null = null;
  let flushing = false;

  function resize(rows: number, cols: number) {
    pending = { dimensions: { rows, cols }, force: pending?.force ?? false };
    void flush();
  }

  function reassert(rows: number, cols: number) {
    pending = { dimensions: { rows, cols }, force: true };
    void flush();
  }

  async function flush() {
    if (flushing || !pending) return;
    const requested = pending;
    const resolved = resolve(requested.dimensions);
    if (!resolved) return;
    if (!requested.force && appliedKey === resolved.key) {
      pending = null;
      return;
    }

    flushing = true;
    let succeeded = false;
    sequence += 1n;
    try {
      await resolved.send(sequence.toString());
      succeeded = true;
      appliedKey = resolved.key;
      if (pending === requested) pending = null;
    } catch {
      // Keep the latest size for the next fit or view reassertion.
    } finally {
      flushing = false;
    }
    if (succeeded && pending) void flush();
  }

  function reset() {
    sequence = 0n;
    appliedKey = null;
  }

  return { resize, reassert, flush, reset };
}
