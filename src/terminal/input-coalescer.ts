/**
 * Serialized, coalescing writer for one Pane's human terminal input.
 *
 * Every keystroke used to become its own IPC that waited for the Core writer
 * ACK, and concurrent requests could reach Core out of client-sequence order.
 * This writer keeps exactly one request in flight per Pane. Input that arrives
 * while a request is in flight is queued; consecutive queued items that were
 * captured under the identical input fence (session, generation, channel or
 * PTY, attachment, view, focus epoch, lease and input epoch) are merged into
 * one request, preserving byte order. Items never merge across a fence change,
 * and a queued batch is re-checked against the current fence right before it is
 * sent: if ownership or generation changed meanwhile, the batch is rejected
 * without being written instead of being redirected.
 *
 * A failed write is delivery-uncertain and non-idempotent, so every batch
 * queued behind it is dropped (rejected with `TerminalInputDroppedError`) rather
 * than sent after an input the user may need to inspect first.
 */
export interface TerminalInputDispatch {
  /** Stable fingerprint of the input fence this dispatch writes under. */
  key: string;
  /** Writes one request under that fence and resolves on the Core writer ACK. Returns the client sequence used. */
  write(value: string): Promise<bigint>;
}

export interface TerminalInputWriteResult {
  /** Client sequence of the request that carried this item, for fenced follow-ups such as history capture. */
  clientSeq: bigint;
  /** Whether this item was the last one in that request. */
  lastInBatch: boolean;
}

/** The input fence changed between queueing and sending; nothing was written. */
export class TerminalInputFenceChangedError extends Error {
  constructor() {
    super("Terminal input fence changed before the queued input was sent");
    this.name = "TerminalInputFenceChangedError";
  }
}

/** An earlier write failed; this queued input was discarded without being written. */
export class TerminalInputDroppedError extends Error {
  constructor() {
    super("Queued terminal input was discarded after an earlier write failed");
    this.name = "TerminalInputDroppedError";
  }
}

/** Requests stay far below the 64 KiB Core frame limit; larger single items are sent alone. */
export const COALESCED_INPUT_MAX_BYTES = 4096;

interface QueuedItem {
  value: string;
  resolve: (result: TerminalInputWriteResult) => void;
  reject: (error: unknown) => void;
}

interface Batch {
  key: string;
  items: QueuedItem[];
  bytes: number;
  sealed: boolean;
}

export interface TerminalInputWriter {
  /**
   * Queues input. `prepare` captures the current fence synchronously; when it
   * returns null the input is rejected immediately, exactly as an unavailable
   * direct send would be. `seal` closes the batch after this item so a fenced
   * follow-up (history capture after Enter) refers to a request ending with it.
   */
  send(value: string, options?: { seal?: boolean }): Promise<TerminalInputWriteResult>;
  /** Rejects queued (not in-flight) input, e.g. when the Pane is torn down or rebinds. */
  discardQueued(): void;
}

const encoder = new TextEncoder();

export function createTerminalInputWriter(
  prepare: () => TerminalInputDispatch | null,
  options: { maxBatchBytes?: number; unavailable?: () => Error } = {},
): TerminalInputWriter {
  const maxBatchBytes = options.maxBatchBytes ?? COALESCED_INPUT_MAX_BYTES;
  const queue: Batch[] = [];
  let inFlight = false;

  function rejectQueued(error: () => unknown) {
    for (const batch of queue.splice(0)) {
      for (const item of batch.items) item.reject(error());
    }
  }

  async function pump() {
    if (inFlight) return;
    const batch = queue.shift();
    if (!batch) return;
    inFlight = true;
    const dispatch = prepare();
    try {
      if (!dispatch || dispatch.key !== batch.key) throw new TerminalInputFenceChangedError();
      const clientSeq = await dispatch.write(batch.items.map((item) => item.value).join(""));
      batch.items.forEach((item, index) => item.resolve({ clientSeq, lastInBatch: index === batch.items.length - 1 }));
    } catch (error) {
      for (const item of batch.items) item.reject(error);
      // Never send later input after a write whose delivery is uncertain or whose fence moved.
      rejectQueued(() => new TerminalInputDroppedError());
    } finally {
      inFlight = false;
    }
    void pump();
  }

  return {
    send(value, sendOptions = {}) {
      const dispatch = prepare();
      if (!dispatch) {
        return Promise.reject(options.unavailable?.() ?? new Error("Terminal input is unavailable"));
      }
      const bytes = encoder.encode(value).length;
      return new Promise<TerminalInputWriteResult>((resolve, reject) => {
        const item = { value, resolve, reject };
        const tail = queue.at(-1);
        if (tail && !tail.sealed && tail.key === dispatch.key && tail.bytes + bytes <= maxBatchBytes) {
          tail.items.push(item);
          tail.bytes += bytes;
          tail.sealed = Boolean(sendOptions.seal);
        } else {
          queue.push({ key: dispatch.key, items: [item], bytes, sealed: Boolean(sendOptions.seal) });
        }
        void pump();
      });
    },
    discardQueued() {
      rejectQueued(() => new TerminalInputDroppedError());
    },
  };
}
