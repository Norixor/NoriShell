<script setup lang="ts">
import { emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";

import { NvxButton, NvxDialog, NvxInlineNotice, NvxTips } from "../components/ui";
import { desktopClient } from "../core-api/desktop-client";
import { disconnectSftpSession, fetchLocalSessionSnapshot, fetchSftpSessionSnapshot, terminateLocalSession } from "../core-api/client";
import { workspaceTabFailureCode as failureCode, workspaceTabFailureMessageKey } from "../workspace-tab-errors";
import type { LocalSessionSummary, NativeTrayAction, PluginApprovedTerminalChannelLaunch, PluginSpecialPermissionOutcome, SftpSessionSummary } from "../core-api/generated/core-api";
import { registerNativeTransferNavigation, type NativeTransferNavigationTarget } from "../native-transfer-navigation";
import { startPluginAppShortcuts } from "../stores/pluginAppIntegrations";
import { i18n } from "../locales";
import { detectDesktopPlatform } from "../platform";
import { invalidatePluginHostDom } from "../plugins/hostDomBroker";
import {
  isEditableShortcutTarget,
  isShortcutExecutionAllowed,
  matchShortcut,
  shouldConsumeShortcut,
  type ShortcutPlatform,
} from "../shortcuts";
import { useShortcutsStore } from "../stores/shortcuts";
import { usePluginExtensionsStore } from "../stores/pluginExtensions";
import { usePluginsStore } from "../stores/plugins";
import { useNativeTerminalStore } from "../stores/nativeTerminal";
import { useUiStore } from "../stores/ui";
import { flushTerminalWorkspaceBeforeExit } from "../terminal-workspace-persistence";
import { useWorkspaceTabsStore, type WorkspacePageTab } from "../stores/workspaceTabs";
import { createDesktopRecovery, parseDesktopHandoff } from "../workspace-desktop-handoff";
import { createFileRecovery } from "../workspace-file-handoff";
import { createPageRecovery } from "../workspace-page-handoff";
import { terminalRecovery, whenAvailable, type WorkspaceTabRecovery } from "../workspace-tab-recovery";
import type { TerminalRestoreSeed } from "../workspace-tab-terminal-restore";
import { createSftpTerminalLaunch } from "./sftpTerminalLaunch";
import { markTabBoot } from "../workspace-tab-boot-trace";
import { afterNextPaint } from "../workspace-tab-paint";
import { startNativeBackgroundSync } from "../native-window-background";
import { acceptSftpPluginNavigation } from "./sftpPluginNavigation";
import { registerWorkspaceTabCloseHandoff } from "../workspace-tab-close-handoff";
import type { TerminalOpenIntent } from "../workspace-tab-view-shell";
import { requestReply } from "../workspace-tab-reply";
import {
  closeOwnWorkspaceTabView as closeOwnNativeTabView,
  getWorkspaceTabContext,
  takeWorkspaceTabBootstrap,
  setWorkspaceTabOwnerWindow,
  snapshotWorkspaceTabs,
  updateOwnWorkspaceTabProjection,
  workspaceTabViewLabel,
  type WorkspaceTabKind,
} from "../workspace-tab-windows";

/** `new` starts from a seed, `recover` rebinds a crash-orphaned Core record. */
interface Bootstrap {
  mode: "new" | "recover" | "unavailable";
  id: string;
  kind: WorkspaceTabKind;
  route: string;
  snapshot?: unknown;
  seed?: unknown;
  ownerWindow: string;
}

interface InitialLocalOpen {
  paneId: string;
  openAttemptId: string;
  operationId: string;
  attachAttemptId: string;
  initialRows: number;
  initialCols: number;
}
type TerminalSeed = { behavior: "welcome"; intent?: TerminalOpenIntent } | { behavior: "local"; initialLocalOpen: InitialLocalOpen }
  | { behavior: "pluginProtocol"; launchId: string; revision: string; paneId: string }
  | { behavior: "pluginApprovedChannel"; payload: PluginApprovedTerminalChannelLaunch }
  | TerminalRestoreSeed;
interface FileSeed { fileKind: "local" | "remote"; hostId: string | null; label: string;
  initialSessionId?: string; initialGeneration?: string; pluginNavigation?: unknown }

interface TabEvent { id: string; batch?: boolean; operationId?: string; replyTo?: string }
interface ContextEvent extends TabEvent { ownerWindow: string }
interface OpenRouteEvent extends TabEvent {
  path: string;
  query?: Record<string, string | string[] | null>;
  directoryPathBytes?: number[];
  transferTarget?: NativeTransferNavigationTarget | null;
}
type OwnedAction = { type: "open-route"; path: string; query: Record<string, string>;
  directoryPathBytes?: number[]; transferTarget?: NativeTransferNavigationTarget | null }
  | { type: "focus-terminal"; action: NativeTrayAction }
  | { type: "quick-connect"; target?: string }
  | { type: "open-desktop"; profileId: string }
  | { type: "telnet" };

const router = useRouter();
const tabs = useWorkspaceTabsStore();
const shortcuts = useShortcutsStore();
const pluginExtensions = usePluginExtensionsStore();
const plugins = usePluginsStore();
const nativeTerminal = useNativeTerminalStore();
const ui = useUiStore();
const tabId = new URLSearchParams(window.location.search).get("tabId") ?? "";
const viewLabel = getCurrentWebview().label;
const ownerWindow = ref(getCurrentWindow().label);
const errorCode = ref("");
const bootstrapped = ref(false);
let permittedRoute = "/workspace-tab-idle";
let recovery: WorkspaceTabRecovery | null = null;
let kind: WorkspaceTabKind | null = null;
let mode: Bootstrap["mode"] | null = null;
const activated = ref(false);
let closeRequested = false;
let closeBusy = false;
let terminalCloseFinalizing = false;
const fallbackFileSessions = ref<SftpSessionSummary[]>([]);
const fallbackFileCloseBusy = ref(false);
const fallbackLocalSession = ref<LocalSessionSummary | null>(null);
const fallbackLocalCloseBusy = ref(false);
let bootstrapPromise: Promise<void> | null = null;
let pluginInvalidated = false;
let initialFileSessionId: string | null = null;
let initialLocalPaneId: string | null = null;
let deferredInitialProjection: unknown | null = null;
const unlisteners: UnlistenFn[] = [];
let stopPluginAppShortcuts: (() => void) | null = null;
let latestProjection: unknown | null = null;
let projectionBusy = false;
let projectionAwaitingContext = false;

// The shell normally answers within a few frames; a lost reply must not stall the close.
const CLOSE_HANDOFF_TIMEOUT_MS = 2_000;

/**
 * Before this view removes its content for a close, the owner shell shows the next
 * Tab (or its own page) and hides this view. Failure only means the teardown may be
 * seen; the close continues either way.
 */
async function handOffBeforeClose(): Promise<void> {
  const operationId = crypto.randomUUID();
  markTabBoot(tabId, "tab", "close_handoff_sent");
  try {
    await syncOwner();
    await requestReply<{ id: string; viewLabel: string; operationId: string; code?: string }>({
      replyEvent: "workspace-tab-view-closing-ready",
      matches: (payload) => payload.id === tabId && payload.viewLabel === viewLabel && payload.operationId === operationId,
      send: () => emitTo(ownerWindow.value, "workspace-tab-view-closing", { id: tabId, viewLabel, operationId }),
      timeoutMs: CLOSE_HANDOFF_TIMEOUT_MS,
      timeoutCode: "workspace_tab.close_handoff_timeout",
    });
    markTabBoot(tabId, "tab", "close_handoff_ready");
  } catch { /* The close proceeds visibly rather than not at all. */ }
}

async function restoreAfterCloseHandoff(): Promise<void> {
  await syncOwner().catch(() => undefined);
  await emitTo(ownerWindow.value, "workspace-tab-view-close-restored", { id: tabId, viewLabel });
}

/** Every close path hides this view before its native WebView is destroyed. */
async function closeOwnWorkspaceTabView(): Promise<void> {
  await handOffBeforeClose();
  await closeOwnNativeTabView();
}

async function syncOwner(): Promise<void> {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      const context = await getWorkspaceTabContext();
      if (context.id !== tabId) throw new Error("workspace_tab.wrong_owner");
      if (ownerWindow.value !== context.ownerWindow) {
        ownerWindow.value = context.ownerWindow;
        setWorkspaceTabOwnerWindow(context.ownerWindow);
        window.dispatchEvent(new CustomEvent("norishell:workspace-tab-context-changed"));
      }
      return;
    } catch (error) {
      if (String(error).includes("workspace_tab.transfer_pending") && attempt < 2) {
        await new Promise((resolve) => window.setTimeout(resolve, 30));
        continue;
      }
      throw error;
    }
  }
}

async function drainProjection(): Promise<void> {
  while (latestProjection !== null) {
    const payload = latestProjection;
    try {
      await updateOwnWorkspaceTabProjection(payload);
      if (kind === "file" && initialFileSessionId && (payload as { panes?: { endpoint?: { sessionId?: string } }[] })
        .panes?.some((pane) => pane.endpoint?.sessionId === initialFileSessionId)) initialFileSessionId = null;
      if (kind === "terminal" && initialLocalPaneId && (payload as { panes?: { paneId?: string; kind?: string; sessionId?: string }[] })
        .panes?.some((pane) => pane.paneId === initialLocalPaneId && pane.kind === "local" && pane.sessionId)) initialLocalPaneId = null;
      if (latestProjection === payload) latestProjection = null;
    } catch (error) {
      const code = failureCode(error);
      if (code === "workspace_tab.transfer_pending") {
        projectionAwaitingContext = true;
        return;
      }
      if (latestProjection === payload) latestProjection = null;
      await syncOwner().catch(() => undefined);
      await emitTo(ownerWindow.value, "workspace-tab-view-projection-failed", {
        id: tabId, viewLabel, code,
      }).catch(() => undefined);
    }
  }
}

function queueProjection(): void {
  if (projectionBusy || projectionAwaitingContext || latestProjection === null) return;
  projectionBusy = true;
  void Promise.resolve().then(drainProjection).finally(() => {
    projectionBusy = false;
    if (!projectionAwaitingContext) queueProjection();
  });
}

function writeProjection(payload: unknown): void {
  if (kind === "terminal" && initialLocalPaneId
    && !(payload as { panes?: { paneId?: string; kind?: string; sessionId?: string }[] })?.panes
      ?.some((pane) => pane.paneId === initialLocalPaneId && pane.kind === "local" && pane.sessionId)) {
    deferredInitialProjection = payload;
    return;
  }
  if (kind === "file" && initialFileSessionId
    && !(payload as { panes?: { endpoint?: { sessionId?: string } }[] })?.panes
      ?.some((pane) => pane.endpoint?.sessionId === initialFileSessionId)) {
    deferredInitialProjection = payload;
    return;
  }
  deferredInitialProjection = null;
  latestProjection = payload;
  queueProjection();
}

function initialResourceSettled(event: Event): void {
  const detail = (event as CustomEvent<{ kind?: string; tabId?: string; resourceId?: string;
    safeToProject?: boolean }>).detail;
  if (!detail || detail.tabId !== tabId) return;
  const matches = (detail.kind === "local" && kind === "terminal" && detail.resourceId === initialLocalPaneId)
    || (detail.kind === "file" && kind === "file" && detail.resourceId === initialFileSessionId);
  if (!matches) return;
  if (!detail.safeToProject) {
    // The Pane owns resource recovery feedback. Retain the seed until it can
    // publish a safe projection, without adding a second Tab-level alert.
    return;
  }
  if (detail.kind === "local") initialLocalPaneId = null;
  else initialFileSessionId = null;
  void nextTick().then(async () => {
    const current = detail.kind === "local"
      ? await tabs.terminalController?.snapshotTabHandoff?.(tabId).catch(() => null) ?? null
      : tabs.fileController(tabId)?.snapshotHandoff() ?? null;
    if (current) writeProjection(current);
    else if (deferredInitialProjection !== null) writeProjection(deferredInitialProjection);
  });
}

watch([() => bootstrapped.value, () => tabs.terminalController], ([ready, controller], _old, onCleanup) => {
  if (!ready || kind !== "terminal" || !controller?.observeTabHandoffSnapshots) return;
  onCleanup(controller.observeTabHandoffSnapshots((snapshots) => {
    const snapshot = snapshots.find((item) => item.tabId === tabId);
    if (snapshot) writeProjection(snapshot);
  }));
}, { immediate: true });

watch([() => bootstrapped.value, () => tabs.fileController(tabId)], ([ready, controller], _old, onCleanup) => {
  if (!ready || kind !== "file" || !controller) return;
  onCleanup(controller.observeHandoffSnapshot((snapshot) => writeProjection(snapshot)));
}, { immediate: true });

watch([() => bootstrapped.value, () => tabs.pageTabs.find((tab) => tab.groupId === tabId)], ([ready, page]) => {
  if (ready && kind === "page" && page) writeProjection({ ...page });
}, { deep: true, immediate: true });

watch([() => activated.value, () => tabs.desktopTabs.find((tab) => tab.groupId === tabId)], ([ready, desktop]) => {
  if (!ready || kind !== "desktop" || !desktop) return;
  try {
    const snapshot = tabs.desktopController?.snapshotHandoff?.(tabId);
    if (snapshot) writeProjection(snapshot);
  } catch { /* The next live session projection will try again. */ }
}, { deep: true, immediate: true });

const removeRouteGuard = router.beforeEach((to) => {
  if (to.path === "/workspace-tab-idle") return;
  if (to.path === permittedRoute) return;
  if (permittedRoute === "/known-hosts" && to.path === "/settings"
    && to.query.section === "knownHosts") return;
  if (permittedRoute === "/settings/identities" && to.path === "/settings"
    && to.query.section === "identities") return;
  // A child renderer owns one Tab. Other routes require the owner shell to create a Tab.
  return false;
});

ui.applyPreferences();
void ui.setUiZoom(ui.uiZoom, false);

function routeFor(kind: WorkspaceTabKind, route: string): string {
  if (kind === "terminal") return "/terminal";
  if (kind === "file") return "/sftp";
  if (kind === "desktop") return "/desktop";
  if (!route.startsWith("/") || route.startsWith("//")) throw new Error("workspace_tab.invalid_page");
  return route;
}

function waitForInitialTerminalController() {
  return whenAvailable(() => tabs.terminalController?.createInitialTab ? tabs.terminalController : null,
    "workspace_tab.terminal_unavailable");
}

async function createInitialTab(input: Bootstrap): Promise<void> {
  if (input.kind === "terminal") {
    const seed = input.seed as Partial<TerminalSeed> | undefined;
    const controller = await waitForInitialTerminalController();
    markTabBoot(tabId, "tab", "controller_available");
    if (seed?.behavior === "welcome") {
      await controller.createInitialTab!(input.id, seed.behavior);
    } else if (seed?.behavior === "local" && seed.initialLocalOpen) {
      initialLocalPaneId = seed.initialLocalOpen.paneId;
      await controller.createInitialTab!(input.id, seed.behavior, seed.initialLocalOpen);
    } else if (seed?.behavior === "pluginProtocol" && seed.launchId && seed.revision && seed.paneId) {
      if (!controller.createInitialPluginProtocolTab) throw new Error("workspace_tab.terminal_unavailable");
      await controller.createInitialPluginProtocolTab(input.id, {
        launchId: seed.launchId, revision: seed.revision, paneId: seed.paneId,
      });
    } else if (seed?.behavior === "pluginApprovedChannel" && seed.payload) {
      if (!controller.createInitialApprovedPluginChannel) throw new Error("workspace_tab.terminal_unavailable");
      await controller.createInitialApprovedPluginChannel(input.id, seed.payload);
    } else if (seed?.behavior === "restore") {
      if (!controller.restoreInitialTab) throw new Error("workspace_tab.terminal_unavailable");
      await controller.restoreInitialTab(input.id, seed as TerminalRestoreSeed);
    } else throw new Error("workspace_tab.invalid_terminal");
    if (!tabs.terminalTabs.some((tab) => tab.groupId === input.id)) {
      throw new Error("workspace_tab.terminal_unavailable");
    }
    return;
  }
  if (input.kind === "file") {
    const seed = input.seed as Partial<FileSeed> | undefined;
    if (!seed || (seed.fileKind !== "local" && seed.fileKind !== "remote")
      || (seed.hostId !== null && typeof seed.hostId !== "string")
      || typeof seed.label !== "string"
      || !tabs.createInitialFileTab(input.id, seed.fileKind, seed.hostId, seed.label,
        seed.initialSessionId, seed.initialGeneration)) {
      throw new Error("workspace_tab.invalid_file");
    }
    if (seed.pluginNavigation && !acceptSftpPluginNavigation(seed.pluginNavigation)) {
      throw new Error("workspace_tab.invalid_plugin_navigation");
    }
    initialFileSessionId = seed.initialSessionId ?? null;
    return;
  }
  if (input.kind === "page") {
    const seed = input.seed as Partial<WorkspacePageTab> | undefined;
    if (!seed || seed.groupId !== input.id || seed.route !== input.route
      || !tabs.importPageTab(seed as WorkspacePageTab)) {
      throw new Error("workspace_tab.invalid_page");
    }
    return;
  }
  // A desktop Tab keeps its identity while its Core session may be replaced by reconnect.
  const desktopSeed = parseDesktopHandoff(input.seed, input.id);
  if (!desktopSeed) throw new Error("workspace_tab.invalid_desktop");
  const desktop = createDesktopRecovery(tabs, router);
  await desktop.import(input.id, desktopSeed);
  recovery = desktop;
}

/**
 * Applies a new Terminal Tab's opening intent before its first display, so it starts in
 * the Host's connecting (or Vault/authentication) step instead of its welcome page.
 * User interaction never blocks this: it continues after the Tab is shown. Returns the
 * stable code of an intent that could not be applied; the Tab itself stays usable.
 */
async function applyTerminalIntent(intent: TerminalOpenIntent | undefined): Promise<string | undefined> {
  if (!intent) return undefined;
  const controller = tabs.terminalController;
  try {
    if (!controller) throw new Error("workspace_tab.terminal_unavailable");
    if (intent.type === "quick-connect") {
      if (!controller.quickConnect(intent.target)) throw new Error("workspace_tab.terminal_unavailable");
    } else if (intent.type === "telnet") {
      if (!controller.openTelnet()) throw new Error("workspace_tab.terminal_unavailable");
    } else if (intent.type === "open-host") {
      if (typeof intent.hostId !== "string" || !intent.hostId || typeof intent.connectOperationId !== "string"
        || !controller.openHost) throw new Error("workspace_tab.open_route_failed");
      let operationId = intent.connectOperationId;
      if (intent.directoryPathBytes !== undefined) {
        if (intent.source !== "sftpDirectory") throw new Error("workspace_tab.invalid_directory");
        const launch = createSftpTerminalLaunch(intent.hostId, intent.directoryPathBytes);
        if (!launch) throw new Error("workspace_tab.invalid_directory");
        operationId = launch;
      }
      await controller.openHost({ hostId: intent.hostId, operationId, source: intent.source,
        pluginAuthorizationToken: intent.pluginAuthorizationToken });
    } else throw new Error("workspace_tab.invalid_action");
    markTabBoot(tabId, "tab", "intent_applied");
    return undefined;
  } catch (error) {
    return failureCode(error, "workspace_tab.open_route_failed");
  }
}

function recoveryFor(kind: WorkspaceTabKind): WorkspaceTabRecovery {
  if (kind === "terminal") return terminalRecovery(tabs, router);
  if (kind === "file") return createFileRecovery(tabs, router) as WorkspaceTabRecovery;
  if (kind === "desktop") return createDesktopRecovery(tabs, router) as WorkspaceTabRecovery;
  return createPageRecovery(tabs, router) as WorkspaceTabRecovery;
}

async function bootstrap(input: Bootstrap): Promise<void> {
  if (!input || input.id !== tabId || viewLabel !== workspaceTabViewLabel(tabId)
    || (input.mode !== "recover" && input.mode !== "new" && input.mode !== "unavailable")
    || !["terminal", "file", "desktop", "page"].includes(input.kind)
    || typeof input.ownerWindow !== "string" || !input.ownerWindow) return;
  if (bootstrapped.value) {
    if (input.kind === kind) {
      await emitTo(ownerWindow.value, "workspace-tab-view-bootstrapped", { id: tabId, viewLabel });
    }
    return;
  }
  if (bootstrapPromise) return bootstrapPromise;
  ownerWindow.value = input.ownerWindow;
  setWorkspaceTabOwnerWindow(input.ownerWindow);
  bootstrapPromise = (async () => {
    let intentCode: string | undefined;
    try {
      kind = input.kind;
      mode = input.mode;
      const path = routeFor(input.kind, input.route);
      permittedRoute = path;
      if (input.mode === "unavailable") {
        errorCode.value = "workspace_tab.orphan_resource_unknown";
        bootstrapped.value = true;
        await emitTo(ownerWindow.value, "workspace-tab-view-bootstrapped", { id: tabId, viewLabel });
        return;
      }
      // Terminal and Desktop register their controllers only after the route mounts.
      if (input.kind === "terminal" || input.kind === "desktop") {
        await router.push(path);
        await nextTick();
        markTabBoot(tabId, "tab", "route_loaded");
      }
      if (input.mode === "recover") {
        const next = recoveryFor(input.kind);
        try { await next.import(tabId, input.snapshot); }
        catch (error) {
          await next.discard(tabId).catch(() => undefined);
          throw error;
        }
        recovery = next;
      } else {
        await createInitialTab(input);
        markTabBoot(tabId, "tab", "initial_tab_created");
        if (input.kind === "terminal" && (input.seed as Partial<TerminalSeed> | undefined)?.behavior === "welcome") {
          // Record the live session handles before the shell can expose a new Terminal Tab.
          const controller = tabs.terminalController;
          if (!controller?.snapshotTabHandoff) throw new Error("workspace_tab.terminal_unavailable");
          await updateOwnWorkspaceTabProjection(await controller.snapshotTabHandoff(tabId));
          intentCode = await applyTerminalIntent((input.seed as { intent?: TerminalOpenIntent }).intent);
        }
      }
      if (pluginInvalidated && input.kind === "page") throw new Error("workspace_tab.plugin_unavailable");
      bootstrapped.value = true;
      errorCode.value = "";
      // The Header shows this Tab's own title before its first display, not after.
      const summary = currentSummary(true);
      if (summary) await emitTo(ownerWindow.value, "workspace-tab-view-summary", summary);
      await emitTo(ownerWindow.value, "workspace-tab-view-bootstrapped", { id: tabId, viewLabel, intentCode });
      markTabBoot(tabId, "tab", "bootstrapped_sent");
    } catch (error) {
      errorCode.value = failureCode(error);
      await emitTo(ownerWindow.value, "workspace-tab-view-bootstrap-failed", {
        id: tabId, viewLabel, code: errorCode.value,
      }).catch(() => undefined);
    } finally {
      bootstrapPromise = null;
      if (pluginInvalidated && kind === "page") void close({ id: tabId });
    }
  })();
  return bootstrapPromise;
}

async function activate(event: TabEvent): Promise<void> {
  if (event?.id !== tabId) return;
  markTabBoot(tabId, "tab", "activate_received");
  await bootstrapPromise;
  if (!bootstrapped.value || !kind || !mode) {
    await emitTo(ownerWindow.value, "workspace-tab-view-activation-failed", {
      id: tabId, viewLabel, operationId: event.operationId, code: "workspace_tab.not_ready",
    }).catch(() => undefined);
    return;
  }
  if (pluginInvalidated && kind === "page") {
    await emitTo(ownerWindow.value, "workspace-tab-view-activation-failed", {
      id: tabId, viewLabel, operationId: event.operationId, code: "workspace_tab.plugin_unavailable",
    }).catch(() => undefined);
    void close({ id: tabId });
    return;
  }
  try {
    await syncOwner();
    if (mode === "unavailable") {
      activated.value = true;
      await emitTo(ownerWindow.value, "workspace-tab-view-activated", { id: tabId, viewLabel, operationId: event.operationId });
      return;
    }
    if (mode === "recover" || kind === "desktop") {
      if (!recovery) throw new Error("workspace_tab.handler_unavailable");
      if (activated.value) await recovery.activateExisting(tabId);
      else await recovery.activate(tabId);
    } else if (kind === "terminal") {
      await router.push("/terminal");
      (await waitForInitialTerminalController()).activate(tabId);
    } else if (kind === "file") {
      if (!tabs.activateFileTab(tabId)) throw new Error("workspace_tab.not_found");
      await router.push("/sftp");
    } else await router.push(permittedRoute);
    activated.value = true;
    errorCode.value = "";
    await emitTo(ownerWindow.value, "workspace-tab-view-activated", { id: tabId, viewLabel, operationId: event.operationId });
    markTabBoot(tabId, "tab", "activated_sent");
  } catch (error) {
    errorCode.value = failureCode(error);
    await emitTo(ownerWindow.value, "workspace-tab-view-activation-failed", {
      id: tabId, viewLabel, operationId: event.operationId, code: errorCode.value,
    }).catch(() => undefined);
  }
}

async function deactivate(event: TabEvent): Promise<void> {
  if (event?.id !== tabId) return;
  let code: string | undefined;
  try {
    if (bootstrapped.value && kind === "terminal"
      && await tabs.terminalController?.deactivate() === false) {
      code = "workspace_tab.input_focus_release_failed";
    } else if (bootstrapped.value && kind === "desktop") tabs.desktopController?.deactivate();
  } catch {
    code = "workspace_tab.deactivation_failed";
  }
  if (event.operationId && event.replyTo) {
    await emitTo(event.replyTo, "workspace-tab-view-deactivated", {
      id: tabId, viewLabel, operationId: event.operationId, code,
    });
  }
}

function contextChanged(event: ContextEvent): void {
  if (event?.id === tabId && typeof event.ownerWindow === "string" && event.ownerWindow) {
    ownerWindow.value = event.ownerWindow;
    setWorkspaceTabOwnerWindow(event.ownerWindow);
    window.dispatchEvent(new CustomEvent("norishell:workspace-tab-context-changed"));
    void syncOwner().catch(() => undefined);
    projectionAwaitingContext = false;
    queueProjection();
  }
}

async function openRoute(event: OpenRouteEvent, propagate = false): Promise<void> {
  if (event?.id !== tabId || !activated.value || event.path !== permittedRoute) return;
  try {
    const query = { ...event.query };
    if (event.transferTarget) {
      if (kind !== "file" || typeof query.focusOperation !== "string") throw new Error("workspace_tab.invalid_transfer");
      registerNativeTransferNavigation(query.focusOperation, event.transferTarget);
    }
    if (event.directoryPathBytes !== undefined) {
      if (kind !== "terminal" || query.source !== "sftpDirectory"
        || typeof query.hostId !== "string") throw new Error("workspace_tab.invalid_directory");
      const operationId = createSftpTerminalLaunch(query.hostId, event.directoryPathBytes);
      if (!operationId) throw new Error("workspace_tab.invalid_directory");
      query.connectOperationId = operationId;
    }
    await router.push({ path: event.path, query });
  } catch (error) {
    await emitTo(ownerWindow.value, "workspace-tab-view-open-route-failed", {
      id: tabId, viewLabel, code: failureCode(error, "workspace_tab.open_route_failed"),
    }).catch(() => undefined);
    if (propagate) throw error;
  }
}

async function runOwnedAction(event: { id: string; action: OwnedAction }): Promise<void> {
  if (event?.id !== tabId || !event.action) throw new Error("workspace_tab.invalid_action");
  await bootstrapPromise;
  for (let attempt = 0; !activated.value && attempt < 100; attempt += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 50));
  }
  if (!activated.value) throw new Error("workspace_tab.activation_timeout");
  const action = event.action;
  if (action.type === "open-route") {
    if (action.path !== permittedRoute) throw new Error("workspace_tab.invalid_route");
    await openRoute({ id: tabId, path: action.path, query: action.query,
      directoryPathBytes: action.directoryPathBytes, transferTarget: action.transferTarget }, true);
    return;
  }
  if (action.type === "open-desktop") {
    if (kind !== "desktop" || !tabs.desktopController) throw new Error("workspace_tab.desktop_unavailable");
    await tabs.desktopController.beginOpenProfile(action.profileId);
    return;
  }
  if (kind !== "terminal" || !tabs.terminalController) throw new Error("workspace_tab.terminal_unavailable");
  if (action.type === "quick-connect") {
    if (!tabs.terminalController.quickConnect(action.target)) throw new Error("workspace_tab.terminal_unavailable");
    return;
  }
  if (action.type === "telnet") {
    if (!tabs.terminalController.openTelnet()) throw new Error("workspace_tab.terminal_unavailable");
    return;
  }
  const target = action.action;
  const controller = tabs.terminalController;
  const focused = target.kind === "focusTerminal" ? controller.focusNativeSession(target.scope)
    : target.kind === "focusSshSession" ? controller.focusSshSession(target.sessionId, target.generation)
      : target.kind === "focusLocalSession" ? controller.focusLocalSession(target.sessionId, target.generation)
        : target.kind === "focusTelnet" ? controller.focusTelnetSession(target.sessionId, target.generation, target.socketId)
          : false;
  if (!focused) throw new Error("workspace_tab.session_unavailable");
}

/** `beforeDisplay` projects a bootstrapped but not yet activated Tab for its first Header title. */
function currentSummary(beforeDisplay = false) {
  if ((!activated.value && !beforeDisplay) || !kind) return null;
  const terminal = tabs.terminalTabs.find((tab) => tab.groupId === tabId);
  if (kind === "terminal" && terminal) return {
    id: tabId, viewLabel, kind, route: permittedRoute, label: terminal.label, stateLabel: terminal.stateLabel,
    hostId: terminal.hostId, bellAttention: terminal.bellAttention, quickCommandsOpen: tabs.quickCommandsOpen,
  };
  const file = tabs.fileTabs.find((tab) => tab.groupId === tabId);
  if (kind === "file" && file) return {
    id: tabId, viewLabel, kind, route: permittedRoute,
    label: file.label || i18n.global.t(file.kind === "remote" ? "fileWorkspace.remoteTab" : "fileWorkspace.localTab"),
    stateLabel: `${i18n.global.t("fileWorkspace.tabState")} · ${i18n.global.t("sshTerminal.paneCount", { count: file.paneCount })}`,
  };
  const desktop = tabs.desktopTabs.find((tab) => tab.groupId === tabId);
  if (kind === "desktop" && desktop) return {
    id: tabId, viewLabel, kind, route: permittedRoute, label: desktop.label, stateLabel: desktop.stateLabel,
  };
  const page = tabs.pageTabs.find((tab) => tab.groupId === tabId);
  if (kind === "page" && page) return {
    id: tabId, viewLabel, kind, route: permittedRoute,
    label: page.labelKey ? i18n.global.t(page.labelKey) : page.label,
    stateLabel: i18n.global.t("workspaceTabs.pageState"),
  };
  return null;
}

async function publishSummary(): Promise<void> {
  const summary = currentSummary();
  if (!summary) return;
  await syncOwner();
  await emitTo(ownerWindow.value, "workspace-tab-view-summary", summary);
}

watch(() => [ownerWindow.value, activated.value, kind, tabs.terminalTabs, tabs.quickCommandsOpen, tabs.fileTabs,
  tabs.desktopTabs, tabs.pageTabs, i18n.global.locale.value], () => {
  void publishSummary().catch(() => undefined);
}, { deep: true, flush: "post" });

watch(() => [
  tabs.terminalTabs.some((tab) => tab.groupId === tabId),
  tabs.fileTabs.some((tab) => tab.groupId === tabId),
  tabs.desktopTabs.some((tab) => tab.groupId === tabId),
  tabs.pageTabs.some((tab) => tab.groupId === tabId),
], (presentByKind) => {
  const present = presentByKind[kind === "terminal" ? 0 : kind === "file" ? 1 : kind === "desktop" ? 2 : 3];
  if (present || !closeRequested || kind === "terminal") return;
  closeRequested = false;
  void closeOwnWorkspaceTabView().catch((error: unknown) => {
    errorCode.value = failureCode(error, "workspace_tab.close_failed");
    void emitTo(ownerWindow.value, "workspace-tab-view-close-failed", {
      id: tabId, viewLabel, code: failureCode(error, "workspace_tab.close_failed"),
    }).catch(() => undefined);
  });
}, { flush: "post" });

async function finalizeTerminalClose(): Promise<void> {
  if (!closeRequested || terminalCloseFinalizing || kind !== "terminal"
    || tabs.terminalTabs.some((tab) => tab.groupId === tabId)) return;
  terminalCloseFinalizing = true;
  try {
    await flushTerminalWorkspaceBeforeExit();
    closeRequested = false;
    await closeOwnWorkspaceTabView();
  } catch (error) {
    closeRequested = false;
    errorCode.value = failureCode(error, "workspace_tab.layout_not_durable");
    await emitTo(ownerWindow.value, "workspace-tab-view-close-failed", {
      id: tabId, viewLabel, code: failureCode(error, "workspace_tab.layout_not_durable"),
    }).catch(() => undefined);
  } finally {
    terminalCloseFinalizing = false;
  }
}

function terminalTabCloseCommitted(event: Event): void {
  const ids = (event as CustomEvent<unknown>).detail;
  if (kind !== "terminal" || !Array.isArray(ids) || !ids.includes(tabId)) return;
  // The Terminal View owns Pane closure. Its committed event may arrive before
  // the Header store projects the removed last Pane/Tab.
  void nextTick().then(() => {
    if (tabs.terminalTabs.some((tab) => tab.groupId === tabId)) return;
    closeRequested = true;
    void finalizeTerminalClose();
  });
}

function terminalTabCloseCancelled(event: Event): void {
  const ids = (event as CustomEvent<unknown>).detail;
  if (closeRequested && Array.isArray(ids) && ids.includes(tabId)) {
    closeRequested = false;
    void reportCloseCancelled();
  }
}

function terminalTabCloseFailed(event: Event): void {
  const detail = (event as CustomEvent<{ tabIds: string[]; code: string }>).detail;
  if (!closeRequested || !detail?.tabIds.includes(tabId)) return;
  closeRequested = false;
  errorCode.value = failureCode(detail.code, "workspace_tab.close_failed");
  void emitTo(ownerWindow.value, "workspace-tab-view-close-failed", {
    id: tabId, viewLabel, code: detail.code,
  }).catch(() => undefined);
}

async function reportCloseCancelled(): Promise<void> {
  await emitTo(ownerWindow.value, "workspace-tab-view-close-cancelled", { id: tabId, viewLabel });
}

function cancelFallbackFileClose(): void {
  if (fallbackFileCloseBusy.value) return;
  fallbackFileSessions.value = [];
  closeRequested = false;
  void reportCloseCancelled().catch(() => undefined);
}

function cancelFallbackLocalClose(): void {
  if (fallbackLocalCloseBusy.value) return;
  fallbackLocalSession.value = null;
  closeRequested = false;
  void reportCloseCancelled().catch(() => undefined);
}

async function offerFallbackFileClose(): Promise<boolean> {
  if (kind !== "file") return false;
  const record = (await snapshotWorkspaceTabs()).owned.find((item) => item.id === tabId && item.kind === "file");
  const payload = record?.payload as { initialSessionId?: unknown; initialGeneration?: unknown;
    panes?: { endpoint?: { kind?: unknown; sessionId?: unknown; generation?: unknown } }[] } | null;
  const expected = new Map<string, string | null>();
  if (typeof payload?.initialSessionId === "string") {
    expected.set(payload.initialSessionId, typeof payload.initialGeneration === "string" ? payload.initialGeneration : null);
  }
  for (const pane of Array.isArray(payload?.panes) ? payload.panes : []) {
    const endpoint = pane.endpoint;
    if (endpoint?.kind === "remote" && typeof endpoint.sessionId === "string"
      && typeof endpoint.generation === "string") expected.set(endpoint.sessionId, endpoint.generation);
  }
  if (!expected.size) return false;
  const live = (await fetchSftpSessionSnapshot()).sessions;
  const active: SftpSessionSummary[] = [];
  for (const [sessionId, generation] of expected) {
    const session = live.find((item) => item.sessionId === sessionId);
    if (!session || session.state === "closed") continue;
    if (generation !== null && session.generation !== generation) {
      // This Tab never attached the newer generation.
      if (initialFileSessionId === sessionId) initialFileSessionId = null;
      continue;
    }
    active.push(session);
  }
  fallbackFileSessions.value = active;
  return active.length > 0;
}

async function confirmFallbackFileClose(): Promise<void> {
  if (!fallbackFileSessions.value.length || fallbackFileCloseBusy.value) return;
  fallbackFileCloseBusy.value = true;
  try {
    for (const session of fallbackFileSessions.value) {
      const result = await disconnectSftpSession({ sessionId: session.sessionId, expectedGeneration: session.generation });
      if (result.state !== "closed") throw new Error("workspace_tab.file_cleanup_incomplete");
    }
    fallbackFileSessions.value = [];
    await closeOwnWorkspaceTabView();
  } catch (error) {
    errorCode.value = failureCode(error, "workspace_tab.file_disconnect_failed");
    await offerFallbackFileClose().catch(() => undefined);
  } finally { fallbackFileCloseBusy.value = false; }
}

async function offerFallbackLocalClose(): Promise<boolean> {
  if (kind !== "terminal") return false;
  const record = (await snapshotWorkspaceTabs()).owned.find((item) => item.id === tabId && item.kind === "terminal");
  const seed = record?.payload as { behavior?: unknown; initialLocalOpen?: { openAttemptId?: unknown } } | null;
  const openAttemptId = seed?.behavior === "local" ? seed.initialLocalOpen?.openAttemptId : null;
  if (typeof openAttemptId !== "string") return false;
  const matches = (await fetchLocalSessionSnapshot()).sessions.filter((item) => item.openAttemptId === openAttemptId);
  if (matches.length > 1) throw new Error("workspace_tab.initial_resource_ambiguous");
  const session = matches[0];
  if (!session) {
    if (initialLocalPaneId) throw new Error("workspace_tab.initial_resource_pending");
    return false;
  }
  if (session.state === "closed" || session.state === "exited") return false;
  fallbackLocalSession.value = session;
  return true;
}

async function closeFallbackDesktopSession(): Promise<void> {
  if (kind !== "desktop") return;
  const record = (await snapshotWorkspaceTabs()).owned.find((item) => item.id === tabId && item.kind === "desktop");
  const seed = parseDesktopHandoff(record?.payload, tabId);
  if (!seed || !("sessionId" in seed)) return;
  const live = (await desktopClient.snapshot()).find((session) =>
    session.id === seed.sessionId && session.generation === seed.generation);
  // Core closes only a session recorded for this Tab.
  if (live) await desktopClient.closeOwned(live);
}

async function confirmFallbackLocalClose(): Promise<void> {
  const session = fallbackLocalSession.value;
  if (!session || fallbackLocalCloseBusy.value) return;
  fallbackLocalCloseBusy.value = true;
  try {
    const matches = (await fetchLocalSessionSnapshot()).sessions.filter((item) => item.openAttemptId === session.openAttemptId);
    if (matches.length !== 1 || matches[0]?.sessionId !== session.sessionId
      || matches[0].generation !== session.generation) throw new Error("workspace_tab.initial_resource_changed");
    const current = matches[0];
    if (current.state !== "closed" && current.state !== "exited") {
      await terminateLocalSession({ sessionId: current.sessionId,
        expectedGeneration: current.generation, expectedStateRevision: current.stateRevision });
    }
    const deadline = Date.now() + 10_000;
    while (true) {
      const exact = (await fetchLocalSessionSnapshot()).sessions.filter((item) => item.openAttemptId === session.openAttemptId);
      if (exact.length !== 1 || exact[0]?.sessionId !== session.sessionId
        || exact[0].generation !== session.generation) throw new Error("workspace_tab.initial_resource_changed");
      if (exact[0].state === "closed" || exact[0].state === "exited") break;
      if (exact[0].state === "failed") throw new Error(exact[0].failureReason?.code === "processCleanupFailed"
        ? "workspace_tab.local_cleanup_failed" : "workspace_tab.local_terminate_failed");
      if (Date.now() >= deadline) throw new Error("workspace_tab.local_termination_pending");
      await new Promise((resolve) => window.setTimeout(resolve, 100));
    }
    fallbackLocalSession.value = null;
    await closeOwnWorkspaceTabView();
  } catch (error) {
    errorCode.value = failureCode(error, "workspace_tab.local_terminate_failed");
  } finally { fallbackLocalCloseBusy.value = false; }
}

async function close(event: TabEvent): Promise<void> {
  if (event?.id !== tabId || closeBusy || !kind) return;
  if (kind !== "desktop") closeRequested = true;
  closeBusy = true;
  try {
    await syncOwner();
    await bootstrapPromise;
    // A batch was already confirmed in the owner window; never offer a second cancellable prompt.
    if (event.batch && (mode === "unavailable" || initialLocalPaneId || initialFileSessionId)) {
      throw new Error("workspace_tab.initial_resource_pending");
    }
    if (mode === "unavailable") {
      closeRequested = false;
      if (await offerFallbackFileClose()) return;
      if (await offerFallbackLocalClose()) return;
      await closeFallbackDesktopSession();
      await closeOwnWorkspaceTabView();
      return;
    }
    if (kind === "desktop" && tabs.desktopController?.isBusy()) {
      throw new Error("workspace_tab.busy");
    }
    closeRequested = true;
    const hasTab = kind === "terminal" ? tabs.terminalTabs.some((tab) => tab.groupId === tabId)
      : kind === "file" ? tabs.fileTabs.some((tab) => tab.groupId === tabId)
        : kind === "desktop" ? tabs.desktopTabs.some((tab) => tab.groupId === tabId)
          : tabs.pageTabs.some((tab) => tab.groupId === tabId);
    if (event.batch && !hasTab && kind !== "page") {
      throw new Error("workspace_tab.initial_resource_pending");
    }
    if (!hasTab) {
      if (kind === "terminal") {
        closeRequested = false;
        if (await offerFallbackLocalClose()) return;
        if (initialLocalPaneId) throw new Error("workspace_tab.initial_resource_pending");
        closeRequested = true;
        await finalizeTerminalClose();
        return;
      }
      closeRequested = false;
      if (await offerFallbackFileClose()) return;
      if (await offerFallbackLocalClose()) return;
      await closeFallbackDesktopSession();
      await closeOwnWorkspaceTabView();
      return;
    }
    if (kind === "file" && initialFileSessionId) {
      closeRequested = false;
      if (await offerFallbackFileClose()) return;
      if (initialFileSessionId) throw new Error("workspace_tab.initial_resource_pending");
    }
    if (kind === "terminal") {
      if (initialLocalPaneId) {
        closeRequested = false;
        if (await offerFallbackLocalClose()) return;
        if (initialLocalPaneId) throw new Error("workspace_tab.initial_resource_pending");
      }
      if (!tabs.terminalController) throw new Error("workspace_tab.terminal_unavailable");
      const accepted = event.batch
        ? tabs.terminalController.closeMany([tabId], true)
        : tabs.terminalController.close(tabId);
      if (!accepted) throw new Error("workspace_tab.busy");
    } else if (kind === "file") {
      const closed = await tabs.requestCloseFileTab(tabId, Boolean(event.batch));
      if (!closed) {
        if (event.batch) throw new Error("workspace_tab.file_close_failed");
        closeRequested = false;
        await reportCloseCancelled();
      }
    } else if (kind === "desktop") {
      if (!tabs.desktopController) throw new Error("workspace_tab.desktop_unavailable");
      await tabs.desktopController.close(tabId);
    } else {
      // Leaving the page releases its plugin target; destroying this WebView closes
      // any context that release could not reach.
      // An idle route renders nothing; the next content is shown before that blank page.
      await handOffBeforeClose();
      await router.replace("/workspace-tab-idle");
      await nextTick();
      if (!tabs.closePageTab(tabId)) throw new Error("workspace_tab.not_found");
    }
  } catch (error) {
    closeRequested = false;
    errorCode.value = failureCode(error, "workspace_tab.close_failed");
    await emitTo(ownerWindow.value, "workspace-tab-view-close-failed", {
      id: tabId, viewLabel, code: failureCode(error, "workspace_tab.close_failed"),
    }).catch(() => undefined);
  } finally {
    closeBusy = false;
  }
}

async function pluginRuntimeInvalidated(payload: { pluginId: string }): Promise<void> {
  if (!payload || typeof payload.pluginId !== "string") return;
  invalidatePluginHostDom(payload.pluginId);
  void plugins.refreshInstalled().catch(() => undefined);
  void pluginExtensions.loadNavigation().catch(() => undefined);
  void pluginExtensions.refreshPluginContributions(payload.pluginId).catch(() => undefined);
  window.dispatchEvent(new CustomEvent("norishell:plugin-runtime-invalidated"));
  if (!tabId.startsWith(`page:plugin:${payload.pluginId}:`)) return;
  if (pluginInvalidated) return;
  pluginInvalidated = true;
  await syncOwner().catch(() => undefined);
  await emitTo(ownerWindow.value, "workspace-tab-view-plugin-invalidated", {
    id: tabId, viewLabel, pluginId: payload.pluginId,
  }).catch(() => undefined);
  if (bootstrapPromise) await bootstrapPromise;
  if (kind === "page") await close({ id: tabId });
}

function pluginRuntimeReady(): void {
  void plugins.refreshInstalled().catch(() => undefined);
  void pluginExtensions.loadNavigation().catch(() => undefined);
  window.dispatchEvent(new CustomEvent("norishell:plugin-runtime-ready"));
}

function pluginPageUnavailable(event: Event): void {
  const pluginId = (event as CustomEvent<unknown>).detail;
  if (typeof pluginId === "string" && pluginId) void pluginRuntimeInvalidated({ pluginId });
}

function pluginSpecialPermissionChanged(payload: PluginSpecialPermissionOutcome): void {
  if (!payload) return;
  if (payload.kind === "installed") {
    invalidatePluginHostDom(payload.plugin.pluginId);
    void pluginExtensions.refreshPluginContributions(payload.plugin.pluginId).catch(() => undefined);
  } else plugins.applySpecialPermissionOutcome(payload);
  window.dispatchEvent(new CustomEvent("norishell:plugin-special-permission-changed", { detail: payload }));
}

function shortcutPlatform(): ShortcutPlatform {
  if (navigator.platform.startsWith("Win")) return "windows";
  return detectDesktopPlatform() === "windows" ? "windows" : "macos";
}

function handleShortcut(event: KeyboardEvent): void {
  const recording = Boolean(document.querySelector(".nvx-shortcut-settings__recorder"));
  if (recording) return;
  const modalOpen = tabs.terminalActivationBlocked || Boolean(document.querySelector("[role='dialog']"));
  if (isEditableShortcutTarget(event.target) && !modalOpen) return;
  const platform = shortcutPlatform();
  const command = matchShortcut(event, platform, shortcuts.bindingsFor(platform));
  const filePane = kind === "file" && [
    "terminal.split-right", "terminal.split-down", "terminal.focus-next-pane",
    "terminal.focus-previous-pane", "terminal.close-pane",
  ].includes(command?.id ?? "");
  const context = {
    terminalActive: activated.value && ((kind === "terminal" && Boolean(tabs.terminalController)) || filePane),
    modalOpen,
    shortcutRecording: recording,
  };
  if (!shouldConsumeShortcut(command, context)) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  if (!command || !isShortcutExecutionAllowed(command, event, context)) return;
  if (command.scope === "terminal") {
    if (kind === "file" && filePane) tabs.runFileShortcut(command.id);
    else if (kind === "terminal") {
      if (command.id === "terminal.toggle-quick-commands") tabs.terminalController?.toggleQuickCommands();
      else tabs.terminalController?.runShortcut?.(command.id);
    }
    return;
  }
  // The shell owns Tab order and app commands; the child owns Pane commands.
  void syncOwner().then(() => emitTo(ownerWindow.value, "workspace-tab-shortcut", {
    id: tabId, viewLabel, commandId: command.id,
  })).catch(() => undefined);
}

let stopNativeBackground: (() => void) | null = null;
let stopCloseHandoff: (() => void) | null = null;
onMounted(async () => {
  if (!tabId || viewLabel !== workspaceTabViewLabel(tabId)) {
    errorCode.value = "workspace_tab.invalid_view";
    return;
  }
  stopNativeBackground = startNativeBackgroundSync("webview");
  stopCloseHandoff = registerWorkspaceTabCloseHandoff({
    closing: (ids) => ids.includes(tabId) ? handOffBeforeClose() : Promise.resolve(),
    restored: (ids) => ids.includes(tabId) ? restoreAfterCloseHandoff() : Promise.resolve(),
  });
  window.addEventListener("keydown", handleShortcut, true);
  window.addEventListener("norishell:terminal-tab-close-committed", terminalTabCloseCommitted);
  window.addEventListener("norishell:terminal-tab-close-cancelled", terminalTabCloseCancelled);
  window.addEventListener("norishell:terminal-tab-close-failed", terminalTabCloseFailed);
  window.addEventListener("norishell:initial-resource-settled", initialResourceSettled);
  stopPluginAppShortcuts = await startPluginAppShortcuts().catch(() => null);
  nativeTerminal.start();
  window.addEventListener("norishell:plugin-page-unavailable", pluginPageUnavailable);
  try {
    unlisteners.push(
      await listen<Bootstrap>("workspace-tab-view-bootstrap", ({ payload }) => {
        if (payload?.id === tabId) markTabBoot(tabId, "tab", "bootstrap_received");
        void bootstrap(payload);
      }),
      await listen<TabEvent>("workspace-tab-view-activate", ({ payload }) => { void activate(payload); }),
      await listen<TabEvent>("workspace-tab-view-deactivate", ({ payload }) => { void deactivate(payload); }),
      await listen<TabEvent>("workspace-tab-view-close", ({ payload }) => { void close(payload); }),
      await listen<TabEvent>("workspace-tab-view-toggle-quick-commands", ({ payload }) => {
        if (payload?.id === tabId && kind === "terminal") tabs.terminalController?.toggleQuickCommands();
      }),
      await listen<TabEvent>("workspace-tab-view-request-summary", ({ payload }) => {
        if (payload?.id === tabId) void publishSummary().catch(() => undefined);
      }),
      await listen<ContextEvent>("workspace-tab-context-changed", ({ payload }) => { contextChanged(payload); }),
      await listen<{ id: string; visible: boolean }>("workspace-tab-view-visibility", ({ payload }) => {
        if (payload.id !== tabId) return;
        if (payload.visible) void afterNextPaint().then(() => markTabBoot(tabId, "tab", "shown_painted"));
        void syncOwner().then(() => {
          window.dispatchEvent(new CustomEvent("norishell:workspace-tab-visibility", { detail: payload.visible }));
        }).catch(() => undefined);
      }),
      await listen<OpenRouteEvent>("workspace-tab-view-open-route", ({ payload }) => { void openRoute(payload); }),
      await listen<{ id: string; action: OwnedAction; operationId?: string; replyTo?: string }>("workspace-tab-view-owned-action", ({ payload }) => {
        void runOwnedAction(payload).then(() => {
          if (payload.operationId && payload.replyTo) return emitTo(payload.replyTo,
            "workspace-tab-activate-owned-result", { id: tabId, operationId: payload.operationId });
        }).catch((error: unknown) => {
          if (!payload.operationId || !payload.replyTo) return;
          return emitTo(payload.replyTo, "workspace-tab-activate-owned-result", {
            id: tabId, operationId: payload.operationId, code: failureCode(error, "workspace_tab.activation_failed"),
          }).catch(() => undefined);
        });
      }),
      await listen<{ id: string; operationId: string }>("workspace-tab-view-flush", ({ payload }) => {
        if (payload.id !== tabId || kind !== "terminal" || !payload.operationId) return;
        void (async () => {
          let code: string | undefined;
          try {
            if (!tabs.terminalController) throw new Error("workspace_tab.terminal_unavailable");
            await flushTerminalWorkspaceBeforeExit();
          } catch (error) { code = failureCode(error, "workspace_tab.flush_failed"); }
          await emitTo("main", "workspace-tab-view-flushed", {
            id: tabId, viewLabel, operationId: payload.operationId, code,
          }).catch(() => undefined);
        })();
      }),
      // Core emits plugin runtime events to main; `listen` targets any label, so
      // every Tab WebView receives them directly without a shell relay.
      await listen<{ pluginId: string }>("plugin-runtime-invalidated", ({ payload }) => { void pluginRuntimeInvalidated(payload); }),
      await listen("plugin-runtime-ready", () => { pluginRuntimeReady(); }),
      await listen<PluginSpecialPermissionOutcome>("plugin-special-permission-changed", ({ payload }) => {
        pluginSpecialPermissionChanged(payload);
      }),
      await listen<{ pluginId: string }>("plugin-settings-changed", ({ payload }) => {
        if (payload?.pluginId) void pluginExtensions.refreshPluginContributions(payload.pluginId);
      }),
    );
    // A new Tab's bootstrap waits in Core; a recovered Tab still receives it after `ready`.
    const pending = await takeWorkspaceTabBootstrap().catch(() => null);
    if (pending) {
      markTabBoot(tabId, "tab", "bootstrap_taken");
      void bootstrap(pending as Bootstrap);
    }
    await emitTo(ownerWindow.value, "workspace-tab-view-ready", { id: tabId, viewLabel });
    markTabBoot(tabId, "tab", "ready_sent");
  } catch {
    errorCode.value = "workspace_tab.listener_unavailable";
  }
});

onBeforeUnmount(() => {
  stopNativeBackground?.();
  stopCloseHandoff?.();
  removeRouteGuard();
  window.removeEventListener("keydown", handleShortcut, true);
  window.removeEventListener("norishell:terminal-tab-close-committed", terminalTabCloseCommitted);
  window.removeEventListener("norishell:terminal-tab-close-cancelled", terminalTabCloseCancelled);
  window.removeEventListener("norishell:terminal-tab-close-failed", terminalTabCloseFailed);
  window.removeEventListener("norishell:initial-resource-settled", initialResourceSettled);
  nativeTerminal.stop();
  stopPluginAppShortcuts?.();
  window.removeEventListener("norishell:plugin-page-unavailable", pluginPageUnavailable);
  for (const stop of unlisteners) stop();
});
</script>

<template>
  <NvxTips />
  <NvxDialog
    plugin-protected
    :model-value="fallbackFileSessions.length > 0"
    :title="i18n.global.t('fileWorkspace.closeTabTitle')"
    :description="i18n.global.t('fileWorkspace.closeTabDescription', { count: fallbackFileSessions.length })"
    :close-label="i18n.global.t('fileWorkspace.closeDialog')"
    :dismissible="!fallbackFileCloseBusy"
    @update:model-value="(open) => { if (!open) cancelFallbackFileClose(); }"
  >
    <NvxInlineNotice
      tone="warning"
      :title="i18n.global.t('fileWorkspace.closeTabWarning')"
    />
    <template #actions>
      <NvxButton
        variant="secondary"
        :disabled="fallbackFileCloseBusy"
        @click="cancelFallbackFileClose"
      >
        {{ i18n.global.t('fileWorkspace.cancel') }}
      </NvxButton>
      <NvxButton
        variant="danger"
        :loading="fallbackFileCloseBusy"
        @click="confirmFallbackFileClose"
      >
        {{ i18n.global.t('fileWorkspace.disconnectAndClose') }}
      </NvxButton>
    </template>
  </NvxDialog>
  <NvxDialog
    plugin-protected
    :model-value="fallbackLocalSession !== null"
    :title="i18n.global.t('sshTerminal.closeActiveTabTitle')"
    :description="i18n.global.t('sshTerminal.closeActiveTabBody', { count: 1 })"
    :close-label="i18n.global.t('fileWorkspace.closeDialog')"
    :dismissible="!fallbackLocalCloseBusy"
    @update:model-value="(open) => { if (!open) cancelFallbackLocalClose(); }"
  >
    <template #actions>
      <NvxButton
        variant="secondary"
        :disabled="fallbackLocalCloseBusy"
        @click="cancelFallbackLocalClose"
      >
        {{ i18n.global.t('fileWorkspace.cancel') }}
      </NvxButton>
      <NvxButton
        variant="danger"
        :loading="fallbackLocalCloseBusy"
        @click="confirmFallbackLocalClose"
      >
        {{ i18n.global.t('sshTerminal.disconnectAndClose') }}
      </NvxButton>
    </template>
  </NvxDialog>
  <main class="workspace-tab-content">
    <RouterView v-slot="{ Component, route }">
      <KeepAlive include="SshTerminalView,DesktopView,FileWorkspaceView,SftpView">
        <component
          :is="Component"
          :key="route.path"
        />
      </KeepAlive>
    </RouterView>
    <NvxInlineNotice
      v-if="errorCode"
      class="workspace-tab-error"
      tone="error"
      :title="i18n.global.t(workspaceTabFailureMessageKey(errorCode))"
    >
      <details>
        <summary>{{ i18n.global.t('workspaceTabError.diagnostics') }}</summary>
        <code>{{ errorCode }}</code>
      </details>
    </NvxInlineNotice>
  </main>
</template>

<style>
html, body, #workspace-tab-app {
  min-width: 0;
  min-height: 0;
  width: 100%;
  height: 100%;
  margin: 0;
}
#workspace-tab-app, .workspace-tab-content {
  display: flex;
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}
.workspace-tab-content { position: relative; }
.workspace-tab-content > :first-child {
  flex: 1 1 auto;
  width: 100%;
  min-width: 0;
  min-height: 0;
}
.workspace-tab-error {
  position: absolute;
  inset: auto 1rem 1rem;
}
</style>
