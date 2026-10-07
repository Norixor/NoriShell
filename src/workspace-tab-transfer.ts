import { workspaceHostMode } from "./platform-host";
import {
  closeWorkspaceWindow, closeEmptyWorkspaceWindow,
  moveWorkspaceTabView, openWorkspaceWindow, snapshotWorkspaceTabs,
  workspaceWindowLabel,
} from "./workspace-tab-windows";

const moveInFlight = new Set<string>();

/** Moving a Tab between windows reparents a native Tab WebView; a single document has none. */
function requireNativeTabs(): void {
  if (workspaceHostMode() !== "nativeTabs") throw new Error("workspace_tab.native_views_unsupported");
}
const openingNewWindow = new Set<string>();

/** Core rechecks emptiness before destroying the child window. */
export async function closeChildWorkspaceWindowIfEmpty(): Promise<void> {
  if (workspaceWindowLabel() === "main" || (await snapshotWorkspaceTabs()).owned.length) return;
  await closeWorkspaceWindow();
}

/** Reparent the live renderer; Core verifies owner and target at the move itself. */
export async function moveWorkspaceTab(
  id: string, target: string, closeSourceWhenEmpty = true,
): Promise<boolean> {
  requireNativeTabs();
  const source = workspaceWindowLabel();
  if (source === target || moveInFlight.has(id)) return false;
  moveInFlight.add(id);
  try {
    await moveWorkspaceTabView(id, target);
    // A failed empty-window check cannot turn a completed reparent into a move failure.
    if (source !== "main" && closeSourceWhenEmpty) {
      void closeChildWorkspaceWindowIfEmpty().catch(() => undefined);
    }
    return true;
  } finally {
    moveInFlight.delete(id);
  }
}

export async function moveWorkspaceTabToNewWindow(
  id: string, position?: { x: number; y: number },
): Promise<void> {
  requireNativeTabs();
  if (moveInFlight.has(id) || openingNewWindow.has(id)) return;
  openingNewWindow.add(id);
  let target: string | null = null;
  try {
    target = await openWorkspaceWindow(position);
    const moved = await moveWorkspaceTab(id, target);
    if (!moved) await closeEmptyWorkspaceWindow(target);
  } catch (error) {
    if (target) await closeEmptyWorkspaceWindow(target).catch(() => undefined);
    throw error;
  } finally {
    openingNewWindow.delete(id);
  }
}
