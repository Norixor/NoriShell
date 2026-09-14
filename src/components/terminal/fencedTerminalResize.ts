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
  * Failures never guess write results; the next fit or new lease replays the latest size.
 */
export function createFencedTerminalResize(
  resolve: (dimensions: TerminalDimensions) => ResolvedResize | null,
) {
  let sequence = 0n;
  let pending: TerminalDimensions | null = null;
  let appliedKey: string | null = null;
  let flushing = false;

  function resize(rows: number, cols: number) {
    pending = { rows, cols };
    void flush();
  }

  async function flush() {
    if (flushing || !pending) return;
    const requested = pending;
    const resolved = resolve(requested);
    if (!resolved) return;
    if (appliedKey === resolved.key) {
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
      if (pending?.rows === requested.rows && pending.cols === requested.cols) pending = null;
    } catch {
      // Keep the latest size for the next fit or focus-lease transition.
    } finally {
      flushing = false;
    }
    if (succeeded && pending) void flush();
  }

  function reset() {
    sequence = 0n;
    appliedKey = null;
  }

  return { resize, flush, reset };
}
