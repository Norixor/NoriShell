import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type ToolWindowExitController = {
  /** The user explicitly chose to retain the draft, including dismissing its confirmation. */
  keepOpen(): Promise<void>;
  dispose(): void;
};

/** Register before displaying an editor so application exit cannot bypass its cleanup. */
export async function registerToolWindowExit(options: {
  requestClose(): Promise<boolean>;
  isBusy(): boolean;
  close(): Promise<void>;
  onError?(): void;
}): Promise<ToolWindowExitController> {
  let attemptId: string | null = null;
  let disposed = false;
  let decision: Promise<boolean> | null = null;
  async function reply(id: string, approved: boolean) {
    await invoke("tool_window_exit_reply", { attemptId: id, approved });
  }
  async function keepOpen() {
    const id = attemptId;
    attemptId = null;
    if (!id || disposed) return;
    try { await reply(id, false); } catch { options.onError?.(); }
  }
  const unlisten = await listen<{ attemptId: string }>("tool-window-exit-requested", async (event) => {
    const id = event.payload.attemptId;
    if (disposed || !id || id === attemptId) return;
    attemptId = id;
    if (options.isBusy()) { await keepOpen(); return; }
    try {
      // A timed-out Core attempt may be retried while a promise-based confirmation
      // remains visible. Reuse that decision rather than opening a second dialog.
      const currentDecision = decision ??= options.requestClose();
      let approved: boolean;
      try { approved = await currentDecision; }
      finally { if (decision === currentDecision) decision = null; }
      if (disposed || attemptId !== id) return;
      if (!approved) {
        // Host confirmation is event-driven; false may mean a visible dirty dialog,
        // not refusal. Its explicit Keep editing action calls keepOpen().
        if (options.isBusy()) await keepOpen();
        return;
      }
      await reply(id, true);
      if (disposed || attemptId !== id) return;
      await options.close();
      attemptId = null;
    } catch {
      await keepOpen();
      options.onError?.();
    }
  });
  return {
    keepOpen,
    dispose() { disposed = true; attemptId = null; unlisten(); },
  };
}
