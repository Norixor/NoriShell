import { parseCoreApiError } from "./core-api/client";
import { i18n } from "./locales";
import { useTipsStore } from "./stores/tips";

/** Only structured, non-secret codes may cross a Tab's diagnostic boundary. */
export function workspaceTabFailureCode(error: unknown, fallback = "workspace_tab.bootstrap_failed"): string {
  const coreError = parseCoreApiError(error);
  const candidate = coreError?.code
    ?? (error instanceof Error ? error.message : typeof error === "string" ? error : "");
  if (!coreError && !/^workspace_(?:tab|window)\./.test(candidate)) return fallback;
  return /^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+$/.test(candidate) && candidate.length <= 120
    ? candidate : fallback;
}

export function workspaceTabFailureMessageKey(code: string): string {
  if (/close|disconnect|terminate/.test(code)) return "workspaceTabError.close";
  if (/projection|durable|flush/.test(code)) return "workspaceTabError.projection";
  if (code === "workspace_tab.listener_unavailable"
    || /bootstrap|invalid_view|orphan|restoration|recovery/.test(code)) return "workspaceTabError.bootstrap";
  return "workspaceTabError.action";
}

/**
 * Reports a managed Tab failure with a localized, actionable message. Raw codes
 * never become user copy; unknown errors fall back to `fallback`.
 */
export function showWorkspaceTabFailure(
  error: unknown,
  scope: string,
  fallback = "workspace_tab.action_failed",
  titleKey = "workspaceTabs.unavailable",
): void {
  const code = workspaceTabFailureCode(error, fallback);
  useTipsStore().show({
    scope,
    tone: "error",
    title: i18n.global.t(titleKey),
    message: i18n.global.t(workspaceTabFailureMessageKey(code)),
  });
}
