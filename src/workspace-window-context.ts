/** The secondary entry point has no authority to restore or replace the global Terminal workspace. */
export function isWorkspaceChildWindow(): boolean {
  return window.location.pathname.endsWith("/workspace-window.html");
}
