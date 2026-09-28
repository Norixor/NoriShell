import { emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { watch } from "vue";
import type { Router } from "vue-router";

import { pluginPageTab, workspacePageTabForRoute, type WorkspacePageTab } from "./stores/workspaceTabs";
import { desktopClient } from "./core-api/desktop-client";
import { listHosts } from "./core-api/client";
import type {
  NativeTrayAction, PluginApprovedTerminalChannelLaunch, PluginNavigationItem, PluginProtocolLaunchSummary,
} from "./core-api/generated/core-api";
import type { NativeTransferNavigationTarget } from "./native-transfer-navigation";
import { createUuidV7 } from "./core-api/ids";
import { i18n } from "./locales";
import { showWorkspaceTabFailure, workspaceTabFailureCode } from "./workspace-tab-errors";
import { registerRemoteTerminalWorkspaceFlush } from "./terminal-workspace-persistence";
import type { DesktopTabHandoffSnapshot } from "./workspace-desktop-handoff";
import type { WorkspaceTabShellAction, WorkspaceTabShellActionEvent } from "./workspace-tab-shell-action";
import { closeChildWorkspaceWindowIfEmpty } from "./workspace-tab-transfer";
import { requestReply, SHELL_ACTION_TIMEOUT_MS, VIEW_REPLY_TIMEOUT_MS } from "./workspace-tab-reply";
import { markTabBoot } from "./workspace-tab-boot-trace";
import { afterNextPaint } from "./workspace-tab-paint";
import {
  closeWorkspaceTabView,
  createWorkspaceTabView,
  getWorkspaceTabView,
  focusWorkspaceTabView,
  focusWorkspaceWindowTarget,
  setWorkspaceTabViewBounds,
  setWorkspaceTabViewVisible,
  snapshotWorkspaceTabs,
  workspaceTabViewLabel,
  workspaceWindowLabel,
  type WorkspaceTabKind,
  type WorkspaceTabRecord,
  type WorkspaceTabViewBounds,
} from "./workspace-tab-windows";
import {
  activeWorkspaceTabViewId,
  replaceWorkspaceTabViewSummary,
  retainWorkspaceTabViewSummaries,
  pendingWorkspaceTabViewId,
  setActiveWorkspaceTabView,
  setPendingWorkspaceTabView,
  setWorkspaceTabViewSummary,
  workspaceTabViewFallbackAfterRemoval,
  workspaceTabViewSummaries,
  workspaceTabViewSummary,
  type WorkspaceTabViewSummary,
} from "./workspace-tab-view-state";

type ViewSignal = { id: string; viewLabel: string; code?: string; intentCode?: string };
/**
 * What a new Terminal Tab opens during its bootstrap, before it is first shown, so it
 * never passes through its welcome page. Carried only in the one-time bootstrap, never
 * in the Core Tab record: it may hold a one-time plugin authorization token.
 */
export type TerminalOpenIntent =
  | { type: "open-host"; hostId: string; connectOperationId: string; source?: string;
    pluginAuthorizationToken?: string; directoryPathBytes?: number[] }
  | { type: "quick-connect"; target?: string }
  | { type: "telnet" };
type SignalName = "ready" | "bootstrapped" | "bootstrap-failed";
type MainHostNavigation = { operationId: string; sourceWindow: string; create: boolean };
type ViewReply = ViewSignal & { operationId?: string };
type InitialSummary = Pick<WorkspaceTabViewSummary, "label" | "stateLabel"> & { hostId?: string | null };
/** Where a created Tab appears: `replaces` swaps a Tab (the New page) in place once the target renders. */
export interface WorkspaceTabPlacement { replaces?: string }

const SHELL_ROUTES = ["/terminal", "/sftp", "/hosts", "/overview", "/tunnels", "/settings", "/plugins", "/desktop"];
const signals = new Map<string, ViewSignal>();
const waiters = new Map<string, (signal: ViewSignal) => void>();
const viewLabels = new Map<string, string>();
const creatingTabIds = new Set<string>();
// A bootstrap intent that could not be applied; the created Tab stays and the creator reports it.
const intentFailures = new Map<string, string>();
const recoveringTabIds = new Set<string>();
const pendingCloseRequests = new Map<string, Promise<boolean>>();
// Tabs already swapped out of the Header while their native view closes.
const replacedTabIds = new Set<string>();
// Tabs whose view handed off its screen before tearing its content down for a close.
const closingTabIds = new Set<string>();
let shellRouter: Router | null = null;
let creatingNewPage: Promise<boolean> | null = null;
let contentBounds: WorkspaceTabViewBounds | null = null;
let visibilityWrite = Promise.resolve();
let activationWrite = Promise.resolve();
let refreshRevision = 0;
let shellRouteSettled: (() => Promise<void>) | null = null;
let shellReady: (() => void) | null = null;
const shellStarted = new Promise<void>((resolve) => { shellReady = resolve; });

async function awaitShellStarted(): Promise<void> {
  await Promise.race([
    shellStarted,
    new Promise<never>((_resolve, reject) => window.setTimeout(() => reject(new Error("workspace_tab.manager_unavailable")), 10_000)),
  ]);
}

function signalKey(name: SignalName, id: string): string { return `${name}:${id}`; }

function forgetSignals(id: string): void {
  for (const name of ["ready", "bootstrapped", "bootstrap-failed"] as const) {
    signals.delete(signalKey(name, id));
  }
}

function receiveSignal(name: SignalName, signal: ViewSignal): void {
  if (!signal?.id || !signal.viewLabel) return;
  const key = signalKey(name, signal.id);
  signals.set(key, signal);
  waiters.get(key)?.(signal);
}

function waitForSignal(name: SignalName, id: string, label: string): Promise<ViewSignal> {
  const key = signalKey(name, id);
  const ready = signals.get(key);
  if (ready?.viewLabel === label) return Promise.resolve(ready);
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => {
      waiters.delete(key);
      reject(new Error(`workspace_tab.view_${name}_timeout`));
    }, VIEW_REPLY_TIMEOUT_MS);
    waiters.set(key, (signal) => {
      if (signal.viewLabel !== label) return;
      window.clearTimeout(timer);
      waiters.delete(key);
      resolve(signal);
    });
  });
}

function waitForBootstrap(id: string, label: string): Promise<ViewSignal> {
  return Promise.race([
    waitForSignal("bootstrapped", id, label),
    waitForSignal("bootstrap-failed", id, label).then((failure) => {
      throw new Error(failure.code ?? "workspace_tab.view_bootstrap_failed");
    }),
  ]);
}

/** Hosts is a main-window page; a child window waits until main shows it. */
async function openHostsInMain(query: Record<string, string> | undefined): Promise<void> {
  const operationId = crypto.randomUUID();
  await requestReply<{ operationId: string; code?: string }>({
    replyEvent: "workspace-tab-main-hosts-result",
    matches: (payload) => payload.operationId === operationId,
    send: () => emitTo({ kind: "Webview", label: "main" }, "workspace-tab-main-hosts", {
      operationId, sourceWindow: workspaceWindowLabel(), create: query?.create === "1",
    } satisfies MainHostNavigation),
    timeoutMs: VIEW_REPLY_TIMEOUT_MS,
    timeoutCode: "workspace_tab.main_navigation_timeout",
  });
}

const t = (key: string, values?: Record<string, unknown>) => values ? i18n.global.t(key, values) : i18n.global.t(key);
const paneState = (state: string, count: number) => `${state} · ${t("sshTerminal.paneCount", { count })}`;

/** A generic localized title for a Tab whose content has not reported its own summary yet. */
function fallbackSummary(id: string, kind: WorkspaceTabKind, viewLabel: string, route?: string): WorkspaceTabViewSummary {
  const label = t(kind === "terminal" ? "navigation.terminal" : kind === "file" ? "navigation.sftp"
    : kind === "desktop" ? "desktop.title" : "workspaceTabs.pageState");
  return { id, viewLabel, kind, label, stateLabel: "", route };
}

function routeForRecord(record: WorkspaceTabRecord): string {
  if (record.kind === "terminal") return "/terminal";
  if (record.kind === "file") return "/sftp";
  if (record.kind === "desktop") return "/desktop";
  const route = (record.payload as { route?: unknown } | null)?.route;
  if (typeof route !== "string" || !route.startsWith("/") || route.startsWith("//")) {
    throw new Error("workspace_tab.invalid_page");
  }
  return route;
}

/** A crashed parent destroys its child WebViews; this is an exceptional Core-backed recovery. */
async function restoreOrphanTab(record: WorkspaceTabRecord): Promise<void> {
  if (recoveringTabIds.has(record.id) || creatingTabIds.has(record.id)) return;
  recoveringTabIds.add(record.id);
  try {
    // A replacement WebView uses the same label. Its prior lifecycle signals are invalid.
    forgetSignals(record.id);
    const view = await createWorkspaceTabView(record.id, record.kind, routeForRecord(record), record.payload);
    viewLabels.set(record.id, view.label);
    setWorkspaceTabViewSummary(fallbackSummary(record.id, record.kind, view.label, routeForRecord(record)));
    if (contentBounds) await setWorkspaceTabViewBounds(record.id, contentBounds);
    await waitForSignal("ready", record.id, view.label);
    const seed = record.payload as { behavior?: unknown; fileKind?: unknown; pendingKind?: unknown;
      initialLocalOpen?: { openAttemptId?: string }; initialSessionId?: string } | null;
    // Only seeds with stable Core IDs can be replayed; a replay then reattaches, never recreates.
    const replayable = (record.kind === "terminal" && seed?.behavior === "welcome")
      || (record.kind === "terminal" && seed?.behavior === "local" && Boolean(seed.initialLocalOpen?.openAttemptId))
      || (record.kind === "file" && seed?.fileKind === "remote" && Boolean(seed.initialSessionId));
    const unknown = Boolean(seed?.pendingKind) || (!replayable && (
      (record.kind === "terminal" && seed?.behavior === "local")
      || (record.kind === "file" && (seed?.fileKind === "local" || seed?.fileKind === "remote"))));
    const route = routeForRecord(record);
    await emitTo(view.label, "workspace-tab-view-bootstrap", unknown
      ? { mode: "unavailable", id: record.id, kind: record.kind, route, ownerWindow: workspaceWindowLabel() }
      : replayable
        ? { mode: "new", id: record.id, kind: record.kind, route, seed: record.payload, ownerWindow: workspaceWindowLabel() }
        : { mode: "recover", id: record.id, kind: record.kind, route, snapshot: record.payload, ownerWindow: workspaceWindowLabel() });
    await waitForBootstrap(record.id, view.label);
    await queueActivation(() => activateWorkspaceTabViewNow(record.id));
  } catch (error) {
    showWorkspaceTabFailure(error, `workspace-tab-recover:${record.id}`, "workspace_tab.recovery_failed");
  } finally {
    recoveringTabIds.delete(record.id);
  }
}

function applyVisibility(): Promise<void> {
  // Native show/hide calls for successive Tab selections must not overtake
  // each other. Read the active Tab only when this batch reaches the queue.
  visibilityWrite = visibilityWrite.catch(() => undefined).then(async () => {
    const active = activeWorkspaceTabViewId.value;
    // A pending Tab has no rendered content yet; its creator shows it when ready.
    const pending = pendingWorkspaceTabViewId.value;
    await Promise.all(workspaceTabViewSummaries.value.filter((summary) => summary.id !== pending).map(async (summary) => {
      if (contentBounds) await setWorkspaceTabViewBounds(summary.id, contentBounds);
      const visible = summary.id === active;
      await setWorkspaceTabViewVisible(summary.id, visible);
      await emitTo(summary.viewLabel, "workspace-tab-view-visibility", { id: summary.id, visible });
    }));
  });
  return visibilityWrite;
}

export function setWorkspaceTabViewContentBounds(bounds: WorkspaceTabViewBounds | null): void {
  contentBounds = bounds;
  void applyVisibility().catch(() => undefined);
}

function queueActivation(operation: () => Promise<void>): Promise<void> {
  const result = activationWrite.then(operation, operation);
  activationWrite = result.catch(() => undefined);
  return result;
}

async function deactivateView(id: string): Promise<void> {
  const operationId = createUuidV7();
  const label = workspaceTabViewLabel(id);
  await requestReply<ViewReply>({
    replyEvent: "workspace-tab-view-deactivated",
    matches: (payload) => payload.id === id && payload.viewLabel === label && payload.operationId === operationId,
    send: () => emitTo(label, "workspace-tab-view-deactivate", { id, operationId, replyTo: workspaceWindowLabel() }),
    timeoutMs: VIEW_REPLY_TIMEOUT_MS,
    timeoutCode: "workspace_tab.deactivation_timeout",
  });
}

async function releaseActiveWorkspaceTabView(): Promise<void> {
  const previous = activeWorkspaceTabViewId.value;
  // Summaries are this shell's projection of the views it owns; a moved view is not released here.
  if (previous && workspaceTabViewSummary(previous)) await deactivateView(previous);
}

async function requestViewActivation(id: string, onDispatch?: () => void): Promise<void> {
  const operationId = createUuidV7();
  const label = workspaceTabViewLabel(id);
  await requestReply<ViewReply>({
    replyEvent: "workspace-tab-view-activated",
    failure: { event: "workspace-tab-view-activation-failed", code: "workspace_tab.view_activation_failed" },
    matches: (payload) => payload.id === id && payload.viewLabel === label && payload.operationId === operationId,
    send: async () => {
      // The child may receive this event even if delivery or its reply is lost.
      onDispatch?.();
      await emitTo(label, "workspace-tab-view-activate", { id, operationId });
    },
    timeoutMs: VIEW_REPLY_TIMEOUT_MS,
    timeoutCode: "workspace_tab.view_activated_timeout",
  });
}

/**
 * Shows a view and waits for its activation reply before hiding the previous one;
 * Core rejects views this window does not own.
 */
async function showAndActivate(id: string, onDispatch: () => void, focusWindow = false): Promise<void> {
  await setWorkspaceTabViewVisible(id, true);
  await emitTo(workspaceTabViewLabel(id), "workspace-tab-view-visibility", { id, visible: true });
  await requestViewActivation(id, onDispatch);
  markTabBoot(id, "shell", "activation_reply");
  setActiveWorkspaceTabView(id);
  await applyVisibility();
  if (focusWindow) await focusWorkspaceWindowTarget(workspaceWindowLabel());
  await focusWorkspaceTabView(id);
}

async function restorePreviousWorkspaceTabView(id: string | null): Promise<void> {
  if (id) {
    let activationAttempted = false;
    try {
      await showAndActivate(id, () => { activationAttempted = true; });
      return;
    } catch {
      if (activationAttempted) await deactivateView(id).catch(() => undefined);
      // A failed restore must leave input and the active projection closed.
    }
  }
  setActiveWorkspaceTabView(null);
  await applyVisibility().catch(() => undefined);
}

async function activateWorkspaceTabViewNow(id: string): Promise<void> {
  const previous = activeWorkspaceTabViewId.value;
  const changing = previous !== id;
  let released = false;
  let targetActivationAttempted = false;
  try {
    if (changing && previous) {
      await releaseActiveWorkspaceTabView();
      released = true;
    }
    // A child can need native bounds and visibility to acquire its Pane focus.
    // The Header selection is committed only after its exact activation reply.
    await showAndActivate(id, () => { targetActivationAttempted = true; });
  } catch (error) {
    if (targetActivationAttempted) {
      try { await deactivateView(id); }
      catch {
        setActiveWorkspaceTabView(null);
        await applyVisibility().catch(() => undefined);
        throw error;
      }
    }
    if (released || !previous || previous === id) {
      await restorePreviousWorkspaceTabView(released ? previous : null);
    }
    throw error;
  }
}

export function activateWorkspaceTabView(id: string): Promise<void> {
  return queueActivation(() => activateWorkspaceTabViewNow(id));
}

// A shell page that loads data may take a moment to reveal; the leaving Tab covers it
// meanwhile, but never longer than this before the pending placeholder takes over.
const SHELL_ROUTE_SETTLE_TIMEOUT_MS = 300;

/**
 * The window shell reports when its current route has revealed and painted its page.
 * Without a registration, leaving a Tab waits one painted frame.
 */
export function registerWorkspaceShellRouteSettled(settled: () => Promise<void>): () => void {
  shellRouteSettled = settled;
  return () => { if (shellRouteSettled === settled) shellRouteSettled = null; };
}

async function awaitShellRouteSettled(): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  await Promise.race([
    (shellRouteSettled ?? (() => afterNextPaint()))().catch(() => undefined),
    new Promise<void>((resolve) => { timer = setTimeout(resolve, SHELL_ROUTE_SETTLE_TIMEOUT_MS); }),
  ]);
  if (timer !== undefined) clearTimeout(timer);
}

/**
 * Shows a shell page in place of the active Tab. The Tab releases input first and is
 * deselected at once, but its native view keeps covering the content area while the
 * shell navigates and paints the target page; only then is it hidden. Hiding first
 * would expose the stale shell page and then the blank pending route for a few frames.
 * If input release fails the Tab stays active and nothing navigates.
 */
export function showWorkspaceShellRoute(navigate?: () => unknown): Promise<void> {
  return queueActivation(async () => {
    const previous = activeWorkspaceTabViewId.value;
    if (previous) {
      await releaseActiveWorkspaceTabView();
      markTabBoot(previous, "shell", "leave_released");
      setActiveWorkspaceTabView(null);
    }
    try {
      if (navigate) await navigate();
    } finally {
      if (previous) {
        await awaitShellRouteSettled();
        markTabBoot(previous, "shell", "leave_route_painted");
        await applyVisibility();
        markTabBoot(previous, "shell", "leave_hidden");
      }
    }
  });
}

/**
 * A closing Tab view is about to remove its content (a Terminal's welcome state, an
 * empty page) and later its native WebView. Show the next Tab first (or the shell
 * page when none is left) and hide the closing view, so the teardown is never seen.
 * The closing view's input is released before any other view is granted focus.
 */
function handOffClosingView(id: string): Promise<void> {
  return queueActivation(async () => {
    if (!workspaceTabViewSummary(id)) return;
    closingTabIds.add(id);
    markTabBoot(id, "shell", "close_handoff_start");
    if (activeWorkspaceTabViewId.value === id) {
      const remaining = new Set(workspaceTabViewSummaries.value.map((summary) => summary.id)
        .filter((item) => item !== id && !closingTabIds.has(item)));
      const fallback = workspaceTabViewFallbackAfterRemoval(id, remaining);
      if (fallback) await activateWorkspaceTabViewNow(fallback);
      else {
        await releaseActiveWorkspaceTabView();
        setActiveWorkspaceTabView(null);
      }
    }
    await applyVisibility();
    markTabBoot(id, "shell", "close_handoff_done");
  });
}

/** A handed-off close did not complete; its view shows its restored content again. */
function restoreClosingView(id: string): Promise<void> {
  if (!closingTabIds.delete(id) || !workspaceTabViewSummary(id)) return Promise.resolve();
  return activateWorkspaceTabView(id);
}

export function deactivateWorkspaceTabView(): Promise<void> {
  return queueActivation(async () => {
    if (!activeWorkspaceTabViewId.value) return;
    await releaseActiveWorkspaceTabView();
    setActiveWorkspaceTabView(null);
    await applyVisibility();
  });
}

async function requestCloseWorkspaceTabViewNow(id: string, batch: boolean): Promise<boolean> {
  await activateWorkspaceTabView(id);
  const label = workspaceTabViewLabel(id);
  let resolveClose!: (closed: boolean) => void;
  let rejectClose!: (error: Error) => void;
  const completed = new Promise<boolean>((resolve, reject) => {
    resolveClose = resolve;
    rejectClose = reject;
  });
  const stopChanged = await listen("workspace-tab-state-changed", () => {
    void getWorkspaceTabView(id).then((current) => { if (!current) resolveClose(true); })
      .catch((error: unknown) => rejectClose(new Error(workspaceTabFailureCode(error, "workspace_tab.close_failed"))));
  });
  let stopCancelled: UnlistenFn | null = null;
  let stopFailed: UnlistenFn | null = null;
  let timer: number | undefined;
  try {
    stopCancelled = await listen<ViewSignal>("workspace-tab-view-close-cancelled", ({ payload }) => {
      if (payload.id === id && payload.viewLabel === label) resolveClose(false);
    });
    stopFailed = await listen<ViewSignal>("workspace-tab-view-close-failed", ({ payload }) => {
      if (payload.id === id && payload.viewLabel === label) rejectClose(new Error(payload.code ?? "workspace_tab.close_failed"));
    });
    // A user confirmation may remain open for a while. Once the deadline expires,
    // Core is authoritative: removal is success, a still-owned view is retryable.
    timer = window.setTimeout(() => {
      let lookupTimer: number | undefined;
      void Promise.race([
        getWorkspaceTabView(id),
        new Promise<never>((_resolve, reject) => {
          lookupTimer = window.setTimeout(() => reject(new Error("workspace_tab.close_timeout")), 5_000);
        }),
      ]).then((current) => {
        if (!current) resolveClose(true);
        else rejectClose(new Error("workspace_tab.close_timeout"));
      }).catch((error: unknown) => rejectClose(new Error(workspaceTabFailureCode(error, "workspace_tab.close_timeout"))))
        .finally(() => { if (lookupTimer !== undefined) window.clearTimeout(lookupTimer); });
    }, 60_000);
    await emitTo(label, "workspace-tab-view-close", { id, batch });
    return await completed;
  } finally {
    if (timer !== undefined) window.clearTimeout(timer);
    stopChanged();
    stopCancelled?.();
    stopFailed?.();
  }
}

export function requestCloseWorkspaceTabView(id: string, batch = false): Promise<boolean> {
  const existing = pendingCloseRequests.get(id);
  if (existing) return existing;
  const pending = requestCloseWorkspaceTabViewNow(id, batch);
  pendingCloseRequests.set(id, pending);
  void pending.finally(() => {
    if (pendingCloseRequests.get(id) === pending) pendingCloseRequests.delete(id);
  }).catch(() => undefined);
  return pending;
}

/**
 * The native manager reserves the Tab and creates its WebView before any renderer
 * creates content. The Header selects the new Tab at once with its final title and
 * the shell covers the content area with a placeholder; the new view is shown only
 * after its content has rendered, so neither title nor content passes through a flash.
 */
export async function createManagedWorkspaceTab(
  id: string,
  kind: WorkspaceTabKind,
  route: string,
  seed: unknown,
  payload: unknown,
  initial: InitialSummary | null = null,
  placement: WorkspaceTabPlacement = {},
): Promise<void> {
  return queueActivation(async () => {
    await awaitShellStarted();
    // An existing Tab is only selected; switching between Tabs never uses a placeholder.
    if (workspaceTabViewSummary(id)) {
      await activateWorkspaceTabViewNow(id);
      return;
    }
    if (creatingTabIds.has(id)) return;
    creatingTabIds.add(id);
    const previousActiveId = activeWorkspaceTabViewId.value;
    const replaced = placement.replaces ? workspaceTabViewSummary(placement.replaces) : null;
    const summary = { ...fallbackSummary(id, kind, workspaceTabViewLabel(id), route), ...initial };
    let created = false;
    let bootstrapStarted = false;
    let released = false;
    let activationAttempted = false;
    // Immediate feedback: the new Tab takes its (or the replaced Tab's) Header place and selection.
    if (replaced) {
      replacedTabIds.add(replaced.id);
      replaceWorkspaceTabViewSummary(replaced.id, summary);
    } else setWorkspaceTabViewSummary(summary);
    setPendingWorkspaceTabView(id);
    setActiveWorkspaceTabView(id);
    try {
      if (previousActiveId && previousActiveId !== id) {
        // Revoke the old view's input before the placeholder replaces it.
        await deactivateView(previousActiveId);
        released = true;
        // The opaque placeholder renders under the old view; hide that view only once
        // the placeholder has painted so the shell page never shows in between.
        await afterNextPaint();
        markTabBoot(id, "shell", "placeholder_painted");
      }
      await applyVisibility();
      if (replaced) await setWorkspaceTabViewVisible(replaced.id, false).catch(() => undefined);
      if (!await getWorkspaceTabView(id).catch(() => null)) forgetSignals(id);
      markTabBoot(id, "shell", "create_start");
      // The new view takes this bootstrap from Core at startup instead of waiting for a ready round trip.
      const view = await createWorkspaceTabView(id, kind, route, payload, {
        mode: "new", id, kind, route, seed, ownerWindow: workspaceWindowLabel(),
      });
      markTabBoot(id, "shell", "view_created");
      viewLabels.set(id, view.label);
      created = view.created;
      bootstrapStarted = view.created;
      // xterm measures its first layout from these bounds while the view is still hidden.
      if (contentBounds) await setWorkspaceTabViewBounds(id, contentBounds);
      if (view.created) {
        const ready = await waitForBootstrap(id, view.label);
        if (ready.intentCode) intentFailures.set(id, ready.intentCode);
        markTabBoot(id, "shell", "bootstrapped_received");
      }
      // Shown on top of the placeholder before the placeholder is removed.
      await showAndActivate(id, () => { activationAttempted = true; }, true);
      setPendingWorkspaceTabView(null);
      markTabBoot(id, "shell", "shown");
    } catch (error) {
      setPendingWorkspaceTabView(null);
      if (replaced) {
        replacedTabIds.delete(replaced.id);
        replaceWorkspaceTabViewSummary(id, replaced);
      }
      // A Tab that may own resources stays reachable for recovery; others leave the Header.
      if (bootstrapStarted) setWorkspaceTabViewSummary(summary);
      else retainWorkspaceTabViewSummaries(new Set(workspaceTabViewSummaries.value
        .map((item) => item.id).filter((item) => item !== id)));
      let restorable = true;
      if (activationAttempted) {
        try { await deactivateView(id); }
        catch { restorable = false; }
      }
      if (restorable && (released || !previousActiveId)) await restorePreviousWorkspaceTabView(previousActiveId);
      else {
        setActiveWorkspaceTabView(restorable ? previousActiveId : null);
        await applyVisibility().catch(() => undefined);
      }
      if (!bootstrapStarted && created) await closeWorkspaceTabView(id).catch(() => undefined);
      throw error;
    } finally {
      creatingTabIds.delete(id);
    }
  });
}

export async function createManagedTerminalTab(
  behavior: "welcome" | "local" = "welcome", placement: WorkspaceTabPlacement = {},
  intent: TerminalOpenIntent | null = null, initialSummary: InitialSummary | null = null,
): Promise<string> {
  const id = createUuidV7();
  const seed = behavior === "local" ? { behavior, initialLocalOpen: {
    paneId: createUuidV7(), openAttemptId: createUuidV7(), operationId: createUuidV7(),
    attachAttemptId: createUuidV7(), initialRows: 24, initialCols: 80,
  } } : { behavior };
  // Match the first summary the Terminal View publishes for this seed.
  const initial = initialSummary ?? (behavior === "local"
    ? { label: t("localSession.defaultShell"), stateLabel: paneState(t("localSession.states.starting"), 1) }
    : { label: t("sshTerminal.newTabLabel"), stateLabel: paneState(t("sshTerminal.newTabState"), 1) });
  intentFailures.delete(id);
  // The intent rides only in the bootstrap; Core keeps the plain seed for recovery.
  await createManagedWorkspaceTab(id, "terminal", "/terminal", intent ? { ...seed, intent } : seed, seed, initial, placement);
  const failure = intentFailures.get(id);
  intentFailures.delete(id);
  if (failure) throw new Error(failure);
  return id;
}

/**
 * The Header title a new Terminal for a saved Host starts with. A Host with a ready
 * credential connects at once under its own label; otherwise the Tab asks first.
 */
async function hostTerminalSummary(hostId: string): Promise<InitialSummary | null> {
  const host = (await listHosts().catch(() => [])).find((item) => item.hostId === hostId);
  if (!host?.hasReadyCredential) return null;
  return { label: host.label, stateLabel: paneState(t("sshSession.states.resolving"), 1), hostId };
}

export async function createManagedTerminalForHost(
  query: { hostId: string; source?: string; connectOperationId?: string; pluginAuthorizationToken?: string },
  placement: WorkspaceTabPlacement = {},
): Promise<string> {
  return createManagedTerminalTab("welcome", placement, {
    type: "open-host", hostId: query.hostId, source: query.source,
    connectOperationId: query.connectOperationId ?? createUuidV7(),
    pluginAuthorizationToken: query.pluginAuthorizationToken,
  }, await hostTerminalSummary(query.hostId));
}

export async function createManagedTerminalForDirectory(
  hostId: string, pathBytes: number[], placement: WorkspaceTabPlacement = {},
): Promise<string> {
  return createManagedTerminalTab("welcome", placement, {
    type: "open-host", hostId, source: "sftpDirectory", connectOperationId: createUuidV7(), directoryPathBytes: pathBytes,
  }, await hostTerminalSummary(hostId));
}

export async function createManagedFileTab(
  kind: "local" | "remote", hostId: string | null = null, label = "", pluginNavigation?: unknown,
  placement: WorkspaceTabPlacement = {},
): Promise<string> {
  const id = `file:${crypto.randomUUID()}`;
  const session = (pluginNavigation as { sftpSession?: { sessionId?: unknown; generation?: unknown; hostId?: unknown } } | null)
    ?.sftpSession;
  if (pluginNavigation != null && (kind !== "remote" || typeof session?.sessionId !== "string"
    || !/^[0-9a-f-]{36}$/i.test(session.sessionId) || typeof session.generation !== "string"
    || !/^[0-9]{1,20}$/u.test(session.generation)
    || (session.hostId !== null && typeof session.hostId !== "string"))) {
    throw new Error("workspace_tab.invalid_plugin_navigation");
  }
  const coreSeed = { fileKind: kind, hostId: hostId ?? (session?.hostId as string | null | undefined) ?? null, label,
    ...(session ? { initialSessionId: session.sessionId, initialGeneration: session.generation }
      : kind === "remote" && hostId ? { initialSessionId: createUuidV7() } : {}) };
  const initial = {
    label: label || t(kind === "remote" ? "fileWorkspace.remoteTab" : "fileWorkspace.localTab"),
    stateLabel: paneState(t("fileWorkspace.tabState"), kind === "local" ? 1 : 2),
  };
  await createManagedWorkspaceTab(id, "file", "/sftp", { ...coreSeed, pluginNavigation }, coreSeed, initial, placement);
  return id;
}

export async function createManagedFileForFocus(query: Record<string, string>): Promise<string> {
  const id = await createManagedFileTab(query.focusSessionId ? "remote" : "local");
  await activateOwnedTab(id, workspaceWindowLabel(), { type: "open-route", path: "/sftp", query });
  return id;
}

/** Opens a plugin page as its native Page Tab, or focuses the window whose Tab already shows it. */
export async function openManagedPluginPage(item: PluginNavigationItem): Promise<void> {
  const tab = pluginPageTab(item);
  const owner = (await snapshotWorkspaceTabs()).others.find((record) => record.id === tab.groupId)?.owner;
  if (owner) await focusWorkspaceWindowTarget(owner);
  else await createManagedPageTab(tab);
}

export async function createManagedPageTab(tab: WorkspacePageTab): Promise<void> {
  await createManagedWorkspaceTab(tab.groupId, "page", tab.route, tab, tab,
    { label: tab.labelKey ? t(tab.labelKey) : tab.label, stateLabel: t("workspaceTabs.pageState") });
}

async function createPageForRoute(route: string): Promise<boolean> {
  const tab = workspacePageTabForRoute(route);
  if (!tab) return false;
  if (tab.pageType === "newPage") {
    const current = (await snapshotWorkspaceTabs()).owned.find((record) => record.kind === "page"
      && (record.payload as { route?: unknown } | null)?.route === route);
    if (current) {
      await activateWorkspaceTabView(current.id);
      return true;
    }
    await createManagedPageTab({ ...tab, groupId: `page:newPage:${createUuidV7()}` });
  } else {
    await createManagedPageTab(tab);
  }
  return true;
}

export function createManagedPageForRoute(route: string): Promise<boolean> {
  if (route !== "/new") return createPageForRoute(route);
  if (creatingNewPage) return creatingNewPage;
  const pending = createPageForRoute(route);
  creatingNewPage = pending;
  void pending.finally(() => { if (creatingNewPage === pending) creatingNewPage = null; }).catch(() => undefined);
  return pending;
}

export async function createManagedDesktopForProfile(
  profile: { id: string; label: string }, placement: WorkspaceTabPlacement = {},
): Promise<string> {
  const id = `desktop:${createUuidV7()}`;
  const snapshot: DesktopTabHandoffSnapshot = { schemaVersion: 1, tabId: id, profileId: profile.id };
  await createManagedWorkspaceTab(id, "desktop", "/desktop", snapshot, snapshot,
    { label: profile.label, stateLabel: t("desktop.states.connecting") }, placement);
  try {
    await activateOwnedTab(id, workspaceWindowLabel(), { type: "open-desktop", profileId: profile.id });
  } catch (error) {
    // The new Tab remains visible so its profile can be retried without losing a live connection.
    showWorkspaceTabFailure(error, `workspace-tab-desktop:${id}`, "workspace_tab.activation_failed");
  }
  return id;
}

export async function createManagedPluginProtocolLaunch(launch: PluginProtocolLaunchSummary): Promise<void> {
  if (launch.claimed || launch.expiresAtUnixMs <= Date.now()) return;
  await createManagedWorkspaceTab(launch.tabId, "terminal", "/terminal",
    { behavior: "pluginProtocol", launchId: launch.launchId, revision: launch.revision, paneId: launch.paneId },
    { pendingKind: "pluginProtocol", launchId: launch.launchId, revision: launch.revision, paneId: launch.paneId });
}

export async function createManagedApprovedPluginChannel(payload: PluginApprovedTerminalChannelLaunch): Promise<void> {
  const id = createUuidV7();
  await createManagedWorkspaceTab(id, "terminal", "/terminal",
    { behavior: "pluginApprovedChannel", payload },
    { pendingKind: "pluginApprovedChannel", operationId: payload.operationId });
}

async function activateOwnedTab(id: string, owner: string, action: unknown): Promise<void> {
  const operationId = createUuidV7();
  await requestReply<{ id: string; operationId: string; code?: string }>({
    replyEvent: "workspace-tab-activate-owned-result",
    matches: (payload) => payload.id === id && payload.operationId === operationId,
    send: async () => {
      await focusWorkspaceWindowTarget(owner);
      await emitTo(owner, "workspace-tab-activate-owned", { id, action, operationId, replyTo: workspaceWindowLabel() });
    },
    timeoutMs: SHELL_ACTION_TIMEOUT_MS,
    timeoutCode: "workspace_tab.activation_timeout",
  });
}

function matchingTerminalTab(action: Extract<NativeTrayAction, { kind: "focusTerminal" | "focusSshSession" | "focusLocalSession" | "focusTelnet" }>,
  snapshot: Awaited<ReturnType<typeof snapshotWorkspaceTabs>>): { id: string; owner: string } | null {
  const wanted = action.kind === "focusTerminal"
    ? { kind: action.scope.kind, sessionId: action.scope.sessionId, generation: action.scope.generation }
    : { kind: action.kind === "focusSshSession" ? "ssh" : action.kind === "focusLocalSession" ? "local" : "telnet",
      sessionId: action.sessionId, generation: action.generation };
  const matches = (panes: { kind: string; sessionId: string | null; generation: string | null }[]) =>
    panes.some((pane) => pane.kind === wanted.kind && pane.sessionId === wanted.sessionId && pane.generation === wanted.generation);
  const owned = snapshot.owned.find((tab) => tab.kind === "terminal" && matches(
    Array.isArray((tab.payload as { panes?: unknown } | null)?.panes)
      ? ((tab.payload as { panes: { kind: string; sessionId: string | null; generation: string | null }[] }).panes) : []));
  if (owned) return { id: owned.id, owner: owned.owner };
  const other = snapshot.others.find((tab) => tab.kind === "terminal" && matches(tab.terminalPanes));
  return other ? { id: other.id, owner: other.owner } : null;
}

export async function focusManagedTerminalSession(action: Extract<NativeTrayAction, { kind: "focusTerminal" | "focusSshSession" | "focusLocalSession" | "focusTelnet" }>): Promise<void> {
  const tab = matchingTerminalTab(action, await snapshotWorkspaceTabs());
  if (!tab) throw new Error("workspace_tab.session_unavailable");
  await activateOwnedTab(tab.id, tab.owner, { type: "focus-terminal", action });
}

export async function focusManagedDesktopSession(sessionId: string, generation: string): Promise<void> {
  const live = (await desktopClient.snapshot()).find((session) => session.id === sessionId && session.generation === generation);
  if (!live) throw new Error("workspace_tab.session_unavailable");
  const snapshot = await snapshotWorkspaceTabs();
  const tab = snapshot.owned.find((item) => item.kind === "desktop"
    && (item.payload as { sessionId?: unknown; generation?: unknown } | null)?.sessionId === sessionId
    && (item.payload as { generation?: unknown } | null)?.generation === generation)
    ?? snapshot.others.find((item) => item.kind === "desktop" && item.desktopSessions?.some(
      (session) => session.sessionId === sessionId && session.generation === generation));
  if (!tab) throw new Error("workspace_tab.session_unavailable");
  await activateOwnedTab(tab.id, tab.owner, { type: "open-route", path: "/desktop",
    query: { focusSessionId: sessionId, focusGeneration: generation, focusOperation: createUuidV7() } });
}

function matchingFileTab(sessionId: string, generation: string,
  snapshot: Awaited<ReturnType<typeof snapshotWorkspaceTabs>>): { id: string; owner: string } | null {
  const owned = snapshot.owned.find((tab) => tab.kind === "file" &&
    Array.isArray((tab.payload as { panes?: unknown } | null)?.panes) &&
    (tab.payload as { panes: { endpoint?: { sessionId?: string; generation?: string } }[] }).panes
      .some((pane) => pane.endpoint?.sessionId === sessionId && pane.endpoint?.generation === generation));
  if (owned) return { id: owned.id, owner: owned.owner };
  const other = snapshot.others.find((tab) => tab.kind === "file" && tab.fileSessions?.some(
    (session) => session.sessionId === sessionId && session.generation === generation));
  return other ? { id: other.id, owner: other.owner } : null;
}

export async function focusManagedFileSession(sessionId: string, generation: string): Promise<void> {
  const query = { focusSessionId: sessionId, focusGeneration: generation, focusOperation: createUuidV7() };
  const tab = matchingFileTab(sessionId, generation, await snapshotWorkspaceTabs());
  if (tab) await activateOwnedTab(tab.id, tab.owner, { type: "open-route", path: "/sftp", query });
  else throw new Error("workspace_tab.session_unavailable");
}

export async function openManagedTransferTarget(query: Record<string, string>, target: NativeTransferNavigationTarget | null): Promise<void> {
  const snapshot = await snapshotWorkspaceTabs();
  const fences = target?.kind === "legacy" ? [{ sessionId: target.sessionId, generation: target.generation }]
    : target?.kind === "intent" ? [target.source, target.target].flatMap((fence) =>
      fence.kind === "remoteSession" ? [{ sessionId: fence.sessionId, generation: fence.generation }] : []) : [];
  const owners = fences.map((fence) => matchingFileTab(fence.sessionId, fence.generation, snapshot));
  if (owners.some((owner) => !owner) || new Set(owners.map((owner) => owner?.id)).size > 1) {
    throw new Error("workspace_tab.transfer_owner_unavailable");
  }
  const tab = owners[0] ?? snapshot.owned.find((item) => item.kind === "file")
    ?? snapshot.others.find((item) => item.kind === "file");
  if (tab) {
    await activateOwnedTab(tab.id, tab.owner, { type: "open-route", path: "/sftp", query, transferTarget: target });
  } else {
    const id = await createManagedFileTab("local");
    await activateOwnedTab(id, workspaceWindowLabel(), { type: "open-route", path: "/sftp", query, transferTarget: target });
  }
}

export async function createManagedQuickConnect(target = ""): Promise<void> {
  await createManagedTerminalTab("welcome", {}, { type: "quick-connect", target });
}

export async function createManagedTelnet(): Promise<void> {
  await createManagedTerminalTab("welcome", {}, { type: "telnet" });
}

/** Performs a shell action in this shell window and returns the created Tab, if any. */
export async function performWorkspaceTabShellAction(
  action: WorkspaceTabShellAction, placement: WorkspaceTabPlacement = {},
): Promise<string | null> {
  if (action.type === "navigate") {
    if (!SHELL_ROUTES.includes(action.path)) throw new Error("workspace_tab.invalid_route");
    if (action.path === "/terminal" && action.query?.hostId) {
      return createManagedTerminalForHost(action.query as { hostId: string; source?: string; connectOperationId?: string }, placement);
    }
    if (action.path === "/sftp" && action.query?.hostId) {
      return createManagedFileTab("remote", action.query.hostId, "", undefined, placement);
    }
    if (action.path === "/hosts" && workspaceWindowLabel() !== "main") {
      await openHostsInMain(action.query);
      return null;
    }
    if (!shellRouter) throw new Error("workspace_tab.manager_unavailable");
    await shellRouter.push({ path: action.path, query: action.query });
    return null;
  }
  if (action.type === "new-terminal") return createManagedTerminalTab(action.behavior === "local" ? "local" : "welcome", placement);
  if (action.type === "new-file") {
    return createManagedFileTab(action.kind, action.hostId ?? null, action.label ?? "", undefined, placement);
  }
  if (action.type === "new-desktop") {
    const profile = (await desktopClient.profiles()).find((item) => item.id === action.profileId);
    if (!profile) throw new Error("workspace_tab.desktop_profile_missing");
    return createManagedDesktopForProfile(profile, placement);
  }
  if (action.type === "sftp-directory-terminal") {
    return createManagedTerminalForDirectory(action.hostId, action.pathBytes, placement);
  }
  throw new Error("workspace_tab.invalid_action");
}

export async function startWorkspaceTabViewShell(router: Router): Promise<() => void> {
  shellRouter = router;
  const unlisteners: UnlistenFn[] = [];
  const invalidatedPluginTabs = new Set<string>();
  const stopRemoteFlush = workspaceWindowLabel() === "main"
    ? registerRemoteTerminalWorkspaceFlush(async () => {
      const state = await snapshotWorkspaceTabs();
      const ids = [...state.owned, ...state.others]
        .filter((tab) => tab.kind === "terminal").map((tab) => tab.id);
      if (!ids.length) return;
      const operationId = createUuidV7();
      const pending = new Set(ids);
      await requestReply<ViewReply & { operationId: string }>({
        replyEvent: "workspace-tab-view-flushed",
        matches: (payload) => payload.operationId === operationId
          && payload.viewLabel === workspaceTabViewLabel(payload.id) && pending.has(payload.id),
        settles: (payload) => { pending.delete(payload.id); return pending.size === 0; },
        // Tabs in other windows are addressed by their stable view label.
        send: async () => { await Promise.all(ids.map((id) => emitTo(workspaceTabViewLabel(id), "workspace-tab-view-flush", { id, operationId }))); },
        timeoutMs: VIEW_REPLY_TIMEOUT_MS,
        timeoutCode: "workspace_tab.flush_timeout",
      });
    }) : () => undefined;
  for (const [event, name] of [
    ["workspace-tab-view-ready", "ready"],
    ["workspace-tab-view-bootstrapped", "bootstrapped"],
    ["workspace-tab-view-bootstrap-failed", "bootstrap-failed"],
  ] as const) {
    unlisteners.push(await listen<ViewSignal>(event, ({ payload }) => receiveSignal(name, payload)));
  }
  unlisteners.push(await listen<WorkspaceTabViewSummary>("workspace-tab-view-summary", ({ payload }) => {
    if (viewLabels.get(payload.id) !== payload.viewLabel) return;
    setWorkspaceTabViewSummary(payload);
  }));
  unlisteners.push(await listen<ViewSignal>("workspace-tab-view-projection-failed", ({ payload }) => {
    if (viewLabels.get(payload.id) !== payload.viewLabel) return;
    showWorkspaceTabFailure(payload.code, `workspace-tab-projection:${payload.id}`, "workspace_tab.projection_failed");
  }));
  unlisteners.push(await listen<ViewReply & { replyTo?: string }>("workspace-tab-view-closing", ({ payload }) => {
    if (!payload?.id || viewLabels.get(payload.id) !== payload.viewLabel || !payload.operationId) return;
    void handOffClosingView(payload.id).catch(() => undefined).then(() => emitTo(payload.viewLabel,
      "workspace-tab-view-closing-ready", { id: payload.id, viewLabel: payload.viewLabel, operationId: payload.operationId }))
      .catch(() => undefined);
  }));
  for (const event of ["workspace-tab-view-close-restored", "workspace-tab-view-close-failed", "workspace-tab-view-close-cancelled"]) {
    unlisteners.push(await listen<ViewSignal>(event, ({ payload }) => {
      if (!payload?.id || viewLabels.get(payload.id) !== payload.viewLabel) return;
      void restoreClosingView(payload.id).catch((error: unknown) =>
        showWorkspaceTabFailure(error, `workspace-tab-close-restore:${payload.id}`, "workspace_tab.activation_failed"));
    }));
  }
  unlisteners.push(await listen<ViewSignal & { pluginId: string }>("workspace-tab-view-plugin-invalidated", ({ payload }) => {
    if (viewLabels.get(payload.id) !== payload.viewLabel || !payload.id.startsWith(`page:plugin:${payload.pluginId}:`)) return;
    invalidatedPluginTabs.add(payload.id);
  }));
  if (workspaceWindowLabel() === "main") {
    unlisteners.push(await listen<MainHostNavigation>("workspace-tab-main-hosts", ({ payload }) => {
      if (!payload?.operationId || !payload.sourceWindow) return;
      void (async () => {
        let code: string | undefined;
        try {
          await showWorkspaceShellRoute(() => router.push({ path: "/hosts", query: payload.create ? { create: "1" } : {} }));
          await focusWorkspaceWindowTarget("main");
        } catch {
          code = "workspace_tab.main_navigation_failed";
        }
        await emitTo({ kind: "Webview", label: payload.sourceWindow }, "workspace-tab-main-hosts-result", {
          operationId: payload.operationId, code,
        });
      })().catch(() => undefined);
    }));
  }
  unlisteners.push(await listen<WorkspaceTabShellActionEvent>("workspace-tab-shell-action", ({ payload }) => {
    if (payload?.ownerWindow !== workspaceWindowLabel()) return;
    void (async () => {
      if (!payload.operationId || !payload.action || payload.viewLabel !== workspaceTabViewLabel(payload.id)) {
        throw new Error("workspace_tab.invalid_action");
      }
      // Only the New page replaces itself with the Tab it creates.
      if (payload.replaceSource && !payload.id.startsWith("page:newPage:")) throw new Error("workspace_tab.invalid_source");
      if (!payload.replaceSource) {
        await performWorkspaceTabShellAction(payload.action);
        await emitTo(payload.viewLabel, "workspace-tab-shell-action-result", { id: payload.id, operationId: payload.operationId });
        return;
      }
      let failure: unknown = null;
      try { await performWorkspaceTabShellAction(payload.action, { replaces: payload.id }); }
      catch (error) { failure = error; }
      // Once the target took the source's Header place, the hidden source is always disposed,
      // even when a later step (such as opening a Host in the new Tab) failed.
      const swapped = replacedTabIds.has(payload.id);
      if (failure && !swapped) throw failure;
      // The owner shell completes disposal; a source WebView cannot be relied on after activation changes.
      replacedTabIds.add(payload.id);
      try { await closeWorkspaceTabView(payload.id); }
      finally { replacedTabIds.delete(payload.id); }
      await refreshOwnedViews();
      await closeChildWorkspaceWindowIfEmpty();
      if (failure) showWorkspaceTabFailure(failure, "workspace-tab-action");
    })().catch(async (error: unknown) => {
      const code = workspaceTabFailureCode(error, "workspace_tab.action_failed");
      if (payload.viewLabel && payload.operationId) {
        await emitTo(payload.viewLabel, "workspace-tab-shell-action-result",
          { id: payload.id, operationId: payload.operationId, code }).catch(() => undefined);
      }
      showWorkspaceTabFailure(code, "workspace-tab-action");
    });
  }));

  async function refreshOwnedViews() {
    const revision = ++refreshRevision;
    const previous = new Set(viewLabels.keys());
    const state = await snapshotWorkspaceTabs();
    const ownedRecords = await Promise.all(state.owned.map(async (record) => ({
      record, view: await getWorkspaceTabView(record.id).catch(() => null),
    })));
    if (revision !== refreshRevision) return;
    const ownedViews = ownedRecords.filter((item) => item.view?.ownerWindow === workspaceWindowLabel()
      && !replacedTabIds.has(item.record.id));
    const ids = new Set(ownedViews.map((item) => item.record.id));
    const priorActive = activeWorkspaceTabViewId.value;
    const showingShellPage = ["/hosts", "/overview", "/tunnels", "/settings", "/plugins"].includes(router.currentRoute.value.path);
    // Tabs this shell created already have a summary; only views without one arrived from elsewhere.
    const known = new Set(workspaceTabViewSummaries.value.map((summary) => summary.id));
    const present = new Set([...ids, ...creatingTabIds]);
    const fallbackId = priorActive && !present.has(priorActive) && !showingShellPage
      ? workspaceTabViewFallbackAfterRemoval(priorActive, ids) : null;
    if (priorActive && !present.has(priorActive) && !fallbackId) setActiveWorkspaceTabView(null);
    // A Tab still being created owns its summary and Header position.
    retainWorkspaceTabViewSummaries(new Set([...ids, ...creatingTabIds]));
    for (const { record, view } of ownedViews) {
      if (!view) continue;
      viewLabels.set(record.id, view.label);
      if (!creatingTabIds.has(record.id) && !workspaceTabViewSummaries.value.some((summary) => summary.id === record.id)) {
        let route: string | undefined;
        try { route = routeForRecord(record); } catch { /* An invalid page record keeps no Rail route. */ }
        setWorkspaceTabViewSummary(fallbackSummary(record.id, record.kind, view.label, route));
      }
    }
    for (const id of [...viewLabels.keys()]) if (!ids.has(id)) viewLabels.delete(id);
    for (const id of [...closingTabIds]) if (!ids.has(id)) closingTabIds.delete(id);
    const invalidatedRemoved = [...previous].some((id) => !ids.has(id) && invalidatedPluginTabs.delete(id));
    if (invalidatedRemoved) await router.push("/plugins");
    for (const { record, view } of ownedRecords) {
      if (!view && !creatingTabIds.has(record.id)) void restoreOrphanTab(record);
    }
    const arriving = ownedViews.find(({ record }) => !previous.has(record.id) && !creatingTabIds.has(record.id)
      && !known.has(record.id));
    if (arriving?.view) await emitTo(arriving.view.label, "workspace-tab-view-request-summary", { id: arriving.record.id });
    if (revision !== refreshRevision) return;
    const activateId = arriving?.record.id ?? fallbackId ?? (!activeWorkspaceTabViewId.value && ownedViews.length
      && router.currentRoute.value.path === "/workspace-window" ? ownedViews[0]!.record.id : null);
    if (activateId) await activateWorkspaceTabView(activateId);
    else await applyVisibility();
    if (previous.size && !ids.size) await closeChildWorkspaceWindowIfEmpty();
  }

  unlisteners.push(await listen("workspace-tab-state-changed", () => { void refreshOwnedViews().catch(() => undefined); }));
  unlisteners.push(await listen<{ id: string; action?: unknown; operationId: string; replyTo: string }>("workspace-tab-activate-owned", ({ payload }) => {
    if (!payload?.id || !payload.operationId || !payload.replyTo) return;
    void (async () => {
      if (!payload.action) throw new Error("workspace_tab.invalid_action");
      await activateWorkspaceTabView(payload.id);
      await emitTo(workspaceTabViewLabel(payload.id), "workspace-tab-view-owned-action", payload);
    })().catch(async (error: unknown) => {
      await emitTo(payload.replyTo, "workspace-tab-activate-owned-result", {
        id: payload.id, operationId: payload.operationId,
        code: workspaceTabFailureCode(error, "workspace_tab.activation_failed"),
      }).catch(() => undefined);
    });
  }));
  // Any other shell navigation leaves the active Tab only after the new page painted.
  const stopRoute = watch(() => router.currentRoute.value.fullPath, () => { void showWorkspaceShellRoute().catch(() => undefined); });
  await refreshOwnedViews();
  shellReady?.();
  return () => {
    stopRemoteFlush();
    stopRoute();
    for (const unlisten of unlisteners) unlisten();
    waiters.clear();
    signals.clear();
    if (shellRouter === router) shellRouter = null;
  };
}
