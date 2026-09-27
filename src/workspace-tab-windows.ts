import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type WorkspaceTabKind = "terminal" | "file" | "desktop" | "page";

export interface WorkspaceTabRecord<T = unknown> {
  id: string;
  kind: WorkspaceTabKind;
  owner: string;
  revision: number;
  payload: T;
}

export interface PendingWorkspaceTabTransfer<T = unknown> {
  ticket: string;
  tab: WorkspaceTabRecord<T>;
  source: string;
  target: string;
  phase: "prepared" | "offered" | "ready";
}

export interface WorkspaceTabSnapshot {
  owned: WorkspaceTabRecord[];
  incoming: PendingWorkspaceTabTransfer[];
  outgoing: PendingWorkspaceTabTransfer[];
  others: {
    id: string;
    kind: WorkspaceTabKind;
    owner: string;
    terminalPanes: { paneId: string; kind: string; sessionId: string | null; generation: string | null }[];
  }[];
}

export const workspaceWindowLabel = () => getCurrentWindow().label;

export const openWorkspaceWindow = (position?: { x: number; y: number }) =>
  invoke<string>("workspace_window_open", position);
export const listWorkspaceWindows = () => invoke<string[]>("workspace_window_list");
export const workspaceWindowAtCursor = () => invoke<string | null>("workspace_window_at_cursor");
export const beginWorkspaceTabDrag = (id: string, nonce: string) =>
  invoke<void>("workspace_tab_drag_begin", { id, nonce });
export const cancelWorkspaceTabDrag = (nonce: string) =>
  invoke<void>("workspace_tab_drag_cancel", { nonce });
export const finishWorkspaceTabDrag = (nonce: string) =>
  invoke<void>("workspace_tab_drag_finish", { nonce });
export const focusWorkspaceWindow = () => invoke<void>("workspace_window_focus");
export const focusWorkspaceWindowTarget = (label: string) => invoke<void>("workspace_window_focus", { label });
export const closeWorkspaceWindow = () => invoke<void>("workspace_window_close");
export const closeEmptyWorkspaceWindow = (label: string) => invoke<void>("workspace_window_close_empty", { label });
export const hideWorkspaceWindow = () => getCurrentWindow().hide();
export const showWorkspaceWindow = () => getCurrentWindow().show();

export const registerWorkspaceTab = <T>(tab: { id: string; kind: WorkspaceTabKind; payload: T }) =>
  invoke<WorkspaceTabRecord<T>>("workspace_tab_register", { tab });
export const updateWorkspaceTab = <T>(id: string, expectedRevision: number, payload: T) =>
  invoke<WorkspaceTabRecord<T>>("workspace_tab_update", { id, expectedRevision, payload });
export const unregisterWorkspaceTab = (id: string, expectedRevision: number) =>
  invoke<void>("workspace_tab_unregister", { id, expectedRevision });
export const prepareWorkspaceTabTransfer = <T>(
  id: string,
  target: string,
  expectedRevision: number,
  payload: T,
) => invoke<string>("workspace_tab_prepare", { id, target, expectedRevision, payload });
export const freezeWorkspaceTabTransfer = (ticket: string) =>
  invoke<void>("workspace_tab_source_frozen", { ticket });
export const readyWorkspaceTabTransfer = (ticket: string) =>
  invoke<void>("workspace_tab_target_ready", { ticket });
export const commitWorkspaceTabTransfer = (ticket: string) =>
  invoke<WorkspaceTabRecord>("workspace_tab_commit", { ticket });
export const abortWorkspaceTabTransfer = (ticket: string) =>
  invoke<void>("workspace_tab_abort", { ticket });
export const snapshotWorkspaceTabs = () =>
  invoke<WorkspaceTabSnapshot>("workspace_tab_snapshot");
