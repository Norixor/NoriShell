import { listen } from "@tauri-apps/api/event";

/** Acknowledgements from a Tab WebView for activation, deactivation, flush and navigation. */
export const VIEW_REPLY_TIMEOUT_MS = 20_000;
/** Compound shell work that may create and activate a Tab before replying. */
export const SHELL_ACTION_TIMEOUT_MS = 90_000;

export interface ReplyPayload { code?: string }

/**
 * Sends one request over the Tab event protocol and waits for its correlated reply.
 * The listener is registered before sending so a fast reply cannot be missed. A
 * reply that carries `code` rejects with that stable code; `settles` decides which
 * matching success replies complete the request.
 */
export async function requestReply<T extends ReplyPayload>(options: {
  replyEvent: string;
  /** A separate failure event and the code used when its payload has none. */
  failure?: { event: string; code: string };
  matches(payload: T): boolean;
  settles?(payload: T): boolean;
  send(): Promise<void>;
  timeoutMs: number;
  timeoutCode: string;
}): Promise<T> {
  let resolve!: (payload: T) => void;
  let reject!: (error: Error) => void;
  const reply = new Promise<T>((accept, fail) => { resolve = accept; reject = fail; });
  // A reply may settle while `send` is still pending; only the awaited path reports it.
  reply.catch(() => undefined);
  const stops = [await listen<T>(options.replyEvent, ({ payload }) => {
    if (!payload || !options.matches(payload)) return;
    if (payload.code) reject(new Error(payload.code));
    else if (options.settles?.(payload) ?? true) resolve(payload);
  })];
  let timer: number | undefined;
  try {
    const failure = options.failure;
    if (failure) {
      stops.push(await listen<T>(failure.event, ({ payload }) => {
        if (payload && options.matches(payload)) reject(new Error(payload.code ?? failure.code));
      }));
    }
    timer = window.setTimeout(() => reject(new Error(options.timeoutCode)), options.timeoutMs);
    await options.send();
    return await reply;
  } finally {
    if (timer !== undefined) window.clearTimeout(timer);
    for (const stop of stops) stop();
  }
}
