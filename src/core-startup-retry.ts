/**
 * Android creates the WebView before the Rust `setup` has finished managing its
 * services, so the first Core command of a cold start can be rejected by Tauri with
 * a plain "state not managed" string (not a Core API error). The state exists a
 * moment later; only that specific rejection is retried, and only for a bounded time.
 */
const STATE_NOT_MANAGED = /state not managed for field/;

export function isCoreStartingError(error: unknown): boolean {
  const text = typeof error === "string" ? error : error instanceof Error ? error.message : "";
  return STATE_NOT_MANAGED.test(text);
}

export async function retryWhileCoreStarting<T>(
  run: () => Promise<T>,
  { enabled, maxWaitMs = 20_000, sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)) }:
    { enabled: boolean; maxWaitMs?: number; sleep?: (ms: number) => Promise<void> },
): Promise<T> {
  let waited = 0;
  let delay = 100;
  for (;;) {
    try {
      return await run();
    } catch (error) {
      if (!enabled || !isCoreStartingError(error) || waited >= maxWaitMs) throw error;
      await sleep(delay);
      waited += delay;
      delay = Math.min(delay * 2, 1000);
    }
  }
}
