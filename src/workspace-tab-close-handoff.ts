/**
 * A Tab WebView that is about to remove its own content for a confirmed close first
 * asks its window shell to show the next content and hide this view, so neither the
 * view's empty state nor the shell page underneath flashes before the next Tab.
 * Views call `handOffClosingWorkspaceTabs` right before they tear their content down,
 * and `restoreClosedWorkspaceTabs` when a close they handed off is rolled back.
 * Outside a Tab WebView (the main window's own pages) no handler is registered and
 * both resolve at once. A failed or slow handoff never blocks the close itself.
 */
export interface WorkspaceTabCloseHandoff {
  closing(tabIds: readonly string[]): Promise<void>;
  restored(tabIds: readonly string[]): Promise<void>;
}

let handler: WorkspaceTabCloseHandoff | null = null;

export function registerWorkspaceTabCloseHandoff(next: WorkspaceTabCloseHandoff): () => void {
  handler = next;
  return () => { if (handler === next) handler = null; };
}

export async function handOffClosingWorkspaceTabs(tabIds: readonly string[]): Promise<void> {
  const current = handler;
  if (!current || !tabIds.length) return;
  await current.closing(tabIds).catch(() => undefined);
}

export async function restoreClosedWorkspaceTabs(tabIds: readonly string[]): Promise<void> {
  const current = handler;
  if (!current || !tabIds.length) return;
  await current.restored(tabIds).catch(() => undefined);
}
