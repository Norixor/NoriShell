import { emitTo } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";

import { createUuidV7 } from "./core-api/ids";
import { isWorkspaceTabView } from "./workspace-window-context";
import { getWorkspaceTabContext } from "./workspace-tab-windows";
import { requestReply, SHELL_ACTION_TIMEOUT_MS } from "./workspace-tab-reply";
import { performWorkspaceTabShellAction } from "./workspace-tab-view-shell";

export type WorkspaceTabShellAction =
  | { type: "navigate"; path: string; query?: Record<string, string> }
  | { type: "new-terminal"; behavior?: "default" | "local" }
  | { type: "new-file"; kind: "local" | "remote"; hostId?: string; label?: string }
  | { type: "new-desktop"; profileId: string }
  | { type: "sftp-directory-terminal"; hostId: string; pathBytes: number[] };

export interface WorkspaceTabShellActionEvent {
  id: string;
  viewLabel: string;
  ownerWindow: string;
  operationId: string;
  action: WorkspaceTabShellAction;
  replaceSource?: boolean;
}

/** A Tab WebView cannot create Tabs; it asks its current owner shell and waits for the result. */
export async function requestWorkspaceTabShellAction(
  action: WorkspaceTabShellAction,
  options?: { closeSourceOnSuccess?: boolean },
): Promise<void> {
  const id = new URLSearchParams(window.location.search).get("tabId");
  if (!isWorkspaceTabView() || !id) throw new Error("workspace_tab.invalid_view");
  const context = await getWorkspaceTabContext();
  if (context.id !== id) throw new Error("workspace_tab.wrong_owner");
  const operationId = crypto.randomUUID();
  const viewLabel = getCurrentWebview().label;
  await requestReply<{ id: string; operationId: string; code?: string }>({
    replyEvent: "workspace-tab-shell-action-result",
    matches: (payload) => payload.id === id && payload.operationId === operationId,
    send: () => emitTo({ kind: "Webview", label: context.ownerWindow }, "workspace-tab-shell-action", {
      id, viewLabel, ownerWindow: context.ownerWindow, operationId, action,
      replaceSource: options?.closeSourceOnSuccess,
    } satisfies WorkspaceTabShellActionEvent),
    timeoutMs: SHELL_ACTION_TIMEOUT_MS,
    timeoutCode: "workspace_tab.action_timeout",
  });
}

/** Tab WebViews forward the action to their owner shell; a shell performs it directly. */
export async function runWorkspaceTabShellAction(action: WorkspaceTabShellAction): Promise<void> {
  if (isWorkspaceTabView()) await requestWorkspaceTabShellAction(action);
  else await performWorkspaceTabShellAction(action);
}

export async function openWorkspaceTerminalHost(
  query: { hostId: string; source?: string; connectOperationId?: string },
): Promise<void> {
  await runWorkspaceTabShellAction({ type: "navigate", path: "/terminal", query: {
    ...query, connectOperationId: query.connectOperationId ?? createUuidV7(),
  } });
}

export async function openWorkspaceDirectoryTerminal(hostId: string, pathBytes: number[]): Promise<void> {
  await runWorkspaceTabShellAction({ type: "sftp-directory-terminal", hostId, pathBytes });
}
