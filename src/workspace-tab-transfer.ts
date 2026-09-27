import { emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { readonly, ref } from "vue";

import {
  abortWorkspaceTabTransfer,
  commitWorkspaceTabTransfer,
  closeWorkspaceWindow,
  closeEmptyWorkspaceWindow,
  hideWorkspaceWindow,
  freezeWorkspaceTabTransfer,
  focusWorkspaceWindow,
  openWorkspaceWindow,
  prepareWorkspaceTabTransfer,
  readyWorkspaceTabTransfer,
  registerWorkspaceTab,
  snapshotWorkspaceTabs,
  showWorkspaceWindow,
  unregisterWorkspaceTab,
  updateWorkspaceTab,
  workspaceWindowLabel,
  type PendingWorkspaceTabTransfer,
  type WorkspaceTabKind,
  type WorkspaceTabRecord,
} from "./workspace-tab-windows";

export interface WorkspaceTabHandoff<T = unknown> {
  kind: WorkspaceTabKind;
  owns(id: string): boolean;
  snapshot(id: string): Promise<T>;
  freeze(id: string): Promise<void>;
  import(id: string, snapshot: T): Promise<void>;
  commit(id: string): void | Promise<void>;
  rollback(id: string): Promise<void>;
  discard(id: string): Promise<void>;
  activate(id: string): void | Promise<void>;
  activateExisting(id: string): void | Promise<void>;
}

type TransferOutcome = "ready" | "failed";
const TRANSFER_TIMEOUT_MS = 20_000;
const handlers = new Map<WorkspaceTabKind, WorkspaceTabHandoff>();
const pendingTarget = new Set<string>();
const imported = new Map<string, string>();
const incomingVisualIds = new Set<string>();
const incomingVisualCount = ref(0);
export const visibleIncomingWorkspaceTabs = readonly(incomingVisualCount);
const outgoingVisualIds = ref<ReadonlySet<string>>(new Set());
export const hiddenOutgoingWorkspaceTabs = readonly(outgoingVisualIds);
const moveInFlight = new Set<string>();
const openingNewWindow = new Set<string>();
const readyWaiters = new Map<string, (outcome: TransferOutcome) => void>();
let started = false;
let unlisteners: UnlistenFn[] = [];
let prepareIncomingKind: ((kind: WorkspaceTabKind) => Promise<void>) | null = null;
const missingTimers = new Map<string, number>();
const observedOwnedTabIds = new Set<string>();
let consumeQueue = Promise.resolve();

function showIncoming(id: string): void {
  incomingVisualIds.add(id);
  incomingVisualCount.value = incomingVisualIds.size;
}

function hideIncoming(id: string): void {
  incomingVisualIds.delete(id);
  incomingVisualCount.value = incomingVisualIds.size;
}

export function registerWorkspaceHandoff(handler: WorkspaceTabHandoff): () => void {
  if (handlers.has(handler.kind)) throw new Error(`workspace handoff handler already registered: ${handler.kind}`);
  handlers.set(handler.kind, handler);
  void consumeIncoming();
  return () => {
    if (handlers.get(handler.kind) === handler) handlers.delete(handler.kind);
  };
}

/** Keep the native close guard in sync with tabs created directly in this window. */
export async function syncLocalWorkspaceTabRecords(
  visible: readonly { id: string; kind: WorkspaceTabKind }[],
): Promise<void> {
  const state = await snapshotWorkspaceTabs();
  const present = new Set(visible.map((item) => item.id));
  const inTransfer = new Set([...state.incoming, ...state.outgoing].map((entry) => entry.tab.id));
  for (const item of visible) {
    const missingTimer = missingTimers.get(item.id);
    if (missingTimer !== undefined) window.clearTimeout(missingTimer);
    missingTimers.delete(item.id);
    if (inTransfer.has(item.id)) continue;
    const handler = handlers.get(item.kind);
    if (!handler?.owns(item.id)) continue;
    const owned = state.owned.find((record) => record.id === item.id);
    let payload: unknown;
    try { payload = await handler.snapshot(item.id); } catch {
      if (owned) continue;
      // A connecting Tab still needs a close guard until a safe snapshot is ready.
      payload = { id: item.id };
    }
    if (owned) {
      observedOwnedTabIds.add(item.id);
      if (owned.kind === item.kind && JSON.stringify(owned.payload) !== JSON.stringify(payload)) {
        await updateWorkspaceTab(item.id, owned.revision, payload).catch(() => undefined);
      }
      continue;
    }
    const registered = await registerWorkspaceTab({ id: item.id, kind: item.kind, payload }).catch(() => null);
    if (registered) observedOwnedTabIds.add(item.id);
  }
  for (const record of state.owned) {
    if (present.has(record.id) || inTransfer.has(record.id)) continue;
    // Recovery records imported from another destroyed WebView must remain in Core.
    if (!observedOwnedTabIds.has(record.id)) continue;
    if (missingTimers.has(record.id)) continue;
    missingTimers.set(record.id, window.setTimeout(() => {
      missingTimers.delete(record.id);
      void snapshotWorkspaceTabs().then(async (latest) => {
        const current = latest.owned.find((item) => item.id === record.id);
        if (!current || latest.incoming.some((item) => item.tab.id === record.id)
          || latest.outgoing.some((item) => item.tab.id === record.id)) return;
        if (handlers.get(current.kind)?.owns(record.id)) return;
        const removed = await unregisterWorkspaceTab(record.id, current.revision).then(() => true).catch(() => false);
        if (removed) observedOwnedTabIds.delete(record.id);
      }).catch(() => undefined);
    }, 16_000));
  }
}

async function withHandler(kind: WorkspaceTabKind): Promise<WorkspaceTabHandoff> {
  const deadline = Date.now() + TRANSFER_TIMEOUT_MS;
  while (Date.now() < deadline) {
    const handler = handlers.get(kind);
    if (handler) return handler;
    await new Promise((resolve) => window.setTimeout(resolve, 50));
  }
  throw new Error("workspace_tab.handler_unavailable");
}

function waitForTarget(ticket: string): Promise<TransferOutcome> {
  return new Promise((resolve) => {
    const timeout = window.setTimeout(() => {
      readyWaiters.delete(ticket);
      resolve("failed");
    }, TRANSFER_TIMEOUT_MS);
    readyWaiters.set(ticket, (outcome) => {
      window.clearTimeout(timeout);
      readyWaiters.delete(ticket);
      resolve(outcome);
    });
  });
}

async function ensureSourceRecord(id: string, kind: WorkspaceTabKind, payload: unknown): Promise<WorkspaceTabRecord> {
  const current = (await snapshotWorkspaceTabs()).owned.find((record) => record.id === id);
  if (current) {
    if (current.kind !== kind) throw new Error("workspace_tab.kind_conflict");
    return current;
  }
  return registerWorkspaceTab({ id, kind, payload });
}

/** Move an already existing Tab; the source keeps its snapshot until Core commits ownership. */
export async function moveWorkspaceTab(id: string, kind: WorkspaceTabKind, target: string): Promise<boolean> {
  const source = workspaceWindowLabel();
  if (source === target || moveInFlight.has(id)) return false;
  const handler = await withHandler(kind);
  if (!handler.owns(id)) throw new Error("workspace_tab.not_found");
  moveInFlight.add(id);
  let ticket: string | null = null;
  let frozen = false;
  let committed = false;
  let commitUncertain = false;
  let sourceHidden = false;
  try {
    const payload = await handler.snapshot(id);
    const record = await ensureSourceRecord(id, kind, payload);
    ticket = await prepareWorkspaceTabTransfer(id, target, record.revision, payload);
    const targetResult = waitForTarget(ticket);
    await handler.freeze(id);
    frozen = true;
    outgoingVisualIds.value = new Set([...outgoingVisualIds.value, id]);
    await freezeWorkspaceTabTransfer(ticket);
    const sourceState = await snapshotWorkspaceTabs();
    const otherTab = sourceState.owned.find((record) => record.id !== id
      && !sourceState.outgoing.some((outgoing) => outgoing.tab.id === record.id));
    const sourceActivation = otherTab
      ? Promise.resolve(handlers.get(otherTab.kind)?.activateExisting(otherTab.id))
      : Promise.resolve();
    if (!otherTab && source !== "main") {
      if (sourceState.owned.length === 1 && sourceState.owned[0]?.id === id
        && sourceState.incoming.length === 0 && sourceState.outgoing.length === 1) {
        await hideWorkspaceWindow();
        sourceHidden = true;
      }
    }
    await sourceActivation;
    if (await targetResult !== "ready") throw new Error("workspace_tab.target_unavailable");
    try {
      await commitWorkspaceTabTransfer(ticket);
      committed = true;
    } catch (error) {
      const current = await snapshotWorkspaceTabs().catch(() => null);
      const owner = current?.owned.find((tab) => tab.id === id)?.owner
        ?? current?.others.find((tab) => tab.id === id)?.owner;
      if (owner === target) {
        committed = true;
      } else if (owner === source) {
        throw error;
      } else {
        commitUncertain = true;
        throw new Error("workspace_tab.commit_unknown", { cause: error });
      }
    }
    await handler.commit(id);
    observedOwnedTabIds.delete(id);
    if (source !== "main") {
      const remaining = await snapshotWorkspaceTabs();
      if (!remaining.owned.length && !remaining.incoming.length && !remaining.outgoing.length) {
        await closeWorkspaceWindow();
      }
    }
    return true;
  } catch (error) {
    if (!committed && !commitUncertain) {
      if (ticket) await abortWorkspaceTabTransfer(ticket).catch(() => undefined);
      try {
        if (frozen) await handler.rollback(id);
      } finally {
        if (sourceHidden) await showWorkspaceWindow().catch(() => undefined);
      }
    } else if (sourceHidden) {
      await showWorkspaceWindow().catch(() => undefined);
    }
    throw error;
  } finally {
    outgoingVisualIds.value = new Set([...outgoingVisualIds.value].filter((tabId) => tabId !== id));
    moveInFlight.delete(id);
  }
}

export async function moveWorkspaceTabToNewWindow(
  id: string,
  kind: WorkspaceTabKind,
  position?: { x: number; y: number },
): Promise<void> {
  if (moveInFlight.has(id) || openingNewWindow.has(id)) return;
  openingNewWindow.add(id);
  let target: string | null = null;
  try {
    target = await openWorkspaceWindow(position);
    const moved = await moveWorkspaceTab(id, kind, target);
    if (!moved) await closeEmptyWorkspaceWindow(target);
  } catch (error) {
    if (target) await closeEmptyWorkspaceWindow(target).catch(() => undefined);
    throw error;
  } finally {
    openingNewWindow.delete(id);
  }
}

async function receive(pending: PendingWorkspaceTabTransfer): Promise<void> {
  if (pending.phase !== "offered" || pendingTarget.has(pending.ticket) || pending.target !== workspaceWindowLabel()) return;
  pendingTarget.add(pending.ticket);
  showIncoming(pending.tab.id);
  let handler: WorkspaceTabHandoff | null = null;
  try {
    await prepareIncomingKind?.(pending.tab.kind);
    handler = await withHandler(pending.tab.kind);
    await handler.import(pending.tab.id, pending.tab.payload);
    imported.set(pending.tab.id, pending.ticket);
    await readyWorkspaceTabTransfer(pending.ticket);
  } catch {
    if (handler) await handler.discard(pending.tab.id).catch(() => undefined);
    imported.delete(pending.tab.id);
    hideIncoming(pending.tab.id);
    await emitTo(pending.source, "workspace-tab-import-failed", { ticket: pending.ticket }).catch(() => undefined);
  } finally {
    pendingTarget.delete(pending.ticket);
  }
}

async function consumeIncomingNow(): Promise<void> {
  if (!started) return;
  const state = await snapshotWorkspaceTabs();
  await Promise.all(state.incoming.map(receive));
  const incomingIds = new Set(state.incoming.map((entry) => entry.tab.id));
  const ownedIds = new Set(state.owned.map((entry) => entry.id));
  for (const [id] of imported) {
    if (incomingIds.has(id) || ownedIds.has(id)) continue;
    imported.delete(id);
    hideIncoming(id);
    for (const handler of handlers.values()) {
      await handler.discard(id).catch(() => undefined);
    }
  }
  for (const record of state.owned) {
    const handler = handlers.get(record.kind);
    if (!handler) continue;
    if (imported.has(record.id)) {
      try {
        await handler.activate(record.id);
        imported.delete(record.id);
        hideIncoming(record.id);
        await focusWorkspaceWindow().catch(() => undefined);
      } catch { /* Keep the staged Tab for the next recovery attempt. */ }
      continue;
    }
    if (!handler.owns(record.id)) {
      try {
        await prepareIncomingKind?.(record.kind);
        await handler.import(record.id, record.payload);
        imported.set(record.id, "recovery");
        await handler.activate(record.id);
        imported.delete(record.id);
        hideIncoming(record.id);
        await focusWorkspaceWindow().catch(() => undefined);
      } catch { /* Core retains ownership; the next state change can retry recovery. */ }
    }
  }
}

function consumeIncoming(): Promise<void> {
  consumeQueue = consumeQueue.then(consumeIncomingNow).catch(() => undefined);
  return consumeQueue;
}

export async function startWorkspaceTabTransfers(
  prepareKind: (kind: WorkspaceTabKind) => Promise<void>,
): Promise<() => void> {
  if (started) throw new Error("workspace transfer listeners already started");
  started = true;
  prepareIncomingKind = prepareKind;
  try {
    unlisteners = await Promise.all([
      listen("workspace-tab-offer", () => { void consumeIncoming(); }),
      listen("workspace-tab-state-changed", () => { void consumeIncoming(); }),
      listen<string>("workspace-tab-target-ready", ({ payload }) => {
        readyWaiters.get(payload)?.("ready");
      }),
      listen<{ ticket: string }>("workspace-tab-import-failed", ({ payload }) => {
        readyWaiters.get(payload.ticket)?.("failed");
      }),
    ]);
    await consumeIncoming();
  } catch (error) {
    for (const unlisten of unlisteners) unlisten();
    unlisteners = [];
    started = false;
    prepareIncomingKind = null;
    throw error;
  }
  return () => {
    for (const unlisten of unlisteners) unlisten();
    unlisteners = [];
    started = false;
    prepareIncomingKind = null;
    incomingVisualIds.clear();
    incomingVisualCount.value = 0;
    outgoingVisualIds.value = new Set();
    observedOwnedTabIds.clear();
  };
}
