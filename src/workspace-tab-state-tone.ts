/**
 * Semantic tone of a Workspace Tab's connection state, used wherever the state is
 * shown as a dot next to its text (the phone header pill and Tab switcher). The
 * tone never replaces the text: it is derived from the same Core state the label
 * reads, so the two cannot disagree.
 */
export type WorkspaceTabStateTone = "neutral" | "info" | "success" | "warning" | "danger";

export type WorkspaceTerminalKind = "ssh" | "local" | "telnet" | "plugin" | "launcher";

const RUNNING = new Set(["running", "ready", "connected"]);
const PROGRESS = new Set([
  "resolving", "connecting", "verifyingHostKey", "authenticating", "openingChannel", "automatingLogin",
  "starting", "reconnecting", "opening", "negotiating", "disconnecting", "stopping",
]);
const ATTENTION = new Set(["awaitingHostKeyDecision", "needsAuthentication", "needsInteraction", "lost", "connectionLost"]);
const FAILED = new Set(["failed"]);

export function workspaceTabStateTone(state: string | null | undefined): WorkspaceTabStateTone {
  if (!state) return "neutral";
  if (RUNNING.has(state)) return "success";
  if (FAILED.has(state)) return "danger";
  if (ATTENTION.has(state)) return "warning";
  if (PROGRESS.has(state)) return "info";
  return "neutral";
}
