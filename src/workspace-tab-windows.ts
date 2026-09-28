import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type WorkspaceTabKind = "terminal" | "file" | "desktop" | "page";

/** Tauri WebView labels have a narrower alphabet than business Tab IDs. */
export function workspaceTabViewLabel(id: string): string {
  return `workspace-tab-${btoa(id).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "")}`;
}

export interface WorkspaceTabRecord<T = unknown> {
  id: string;
  kind: WorkspaceTabKind;
  owner: string;
  payload: T;
}

export interface WorkspaceTabSnapshot {
  owned: WorkspaceTabRecord[];
  others: {
    id: string;
    kind: WorkspaceTabKind;
    owner: string;
    terminalPanes: { paneId: string; kind: string; sessionId: string | null; generation: string | null }[];
    fileSessions?: { sessionId: string; generation: string }[];
    desktopSessions?: { sessionId: string; generation: string }[];
  }[];
}

let tabOwnerWindow: string | null = null;

/** A child WebView keeps its initial Tauri window metadata after native reparenting. */
export const setWorkspaceTabOwnerWindow = (label: string) => { tabOwnerWindow = label; };
export const workspaceWindowLabel = () => tabOwnerWindow ?? getCurrentWindow().label;

export interface WorkspaceTabView {
  id: string;
  label: string;
  ownerWindow: string;
  created: boolean;
}

export interface WorkspaceTabContext {
  id: string;
  ownerWindow: string;
}

export const getWorkspaceTabContext = () => invoke<WorkspaceTabContext>("workspace_tab_context_get");

export interface WorkspaceTabViewBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** `bootstrap`, when given, is taken once by the new view itself so it starts without a round trip. */
export const createWorkspaceTabView = (id: string, kind: WorkspaceTabKind, route: string, payload: unknown, bootstrap: unknown = null) =>
  invoke<WorkspaceTabView>("create_tab_view", { id, kind, route, payload, bootstrap });
export const takeWorkspaceTabBootstrap = () => invoke<unknown>("workspace_tab_bootstrap_take");
export const getWorkspaceTabView = (id: string) =>
  invoke<WorkspaceTabView | null>("workspace_tab_view_get", { id });
export const setWorkspaceTabViewBounds = (id: string, bounds: WorkspaceTabViewBounds) =>
  invoke<void>("set_tab_view_bounds", { id, bounds });
export const setWorkspaceTabViewVisible = (id: string, visible: boolean) =>
  invoke<void>("set_tab_view_visible", { id, visible });
export const focusWorkspaceTabView = (id: string) =>
  invoke<void>("focus_tab_view", { id });
export const moveWorkspaceTabView = (id: string, target: string) =>
  invoke<WorkspaceTabView>("move_tab_view", { id, target });
export const closeWorkspaceTabView = (id: string) =>
  invoke<void>("close_tab_view", { id });
/** The child closes itself after its resource-safe close, even if it moved meanwhile. */
export const closeOwnWorkspaceTabView = () => invoke<void>("close_own_tab_view");

export const openWorkspaceWindow = (position?: { x: number; y: number }) =>
  invoke<string>("workspace_window_open", position);
export const beginWorkspaceTabDrag = (id: string, nonce: string) =>
  invoke<void>("workspace_tab_drag_begin", { id, nonce });
export const cancelWorkspaceTabDrag = (nonce: string) =>
  invoke<void>("workspace_tab_drag_cancel", { nonce });
export const finishWorkspaceTabDrag = (nonce: string) =>
  invoke<void>("workspace_tab_drag_finish", { nonce });
export const focusWorkspaceWindowTarget = (label: string) => invoke<void>("workspace_window_focus", { label });
export const closeWorkspaceWindow = () => invoke<void>("workspace_window_close");
export const closeEmptyWorkspaceWindow = (label: string) => invoke<void>("workspace_window_close_empty", { label });

/** The calling Tab WebView updates only its own bounded recovery description. */
export const updateOwnWorkspaceTabProjection = (payload: unknown) =>
  invoke<void>("workspace_tab_projection_update", { payload });
export const snapshotWorkspaceTabs = () =>
  invoke<WorkspaceTabSnapshot>("workspace_tab_snapshot");
