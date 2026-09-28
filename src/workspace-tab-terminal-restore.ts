import {
  fetchLocalSessionSnapshot,
  fetchSshSessionSnapshot,
  fetchTelnetSessionSnapshot,
  fetchTerminalWorkspaceLayout,
  getLocalSession,
  getSshSession,
} from "./core-api/client";
import type { LocalSessionSummary, SshSessionSummary, TelnetSessionSummary } from "./core-api/generated/core-api";
import { createUuidV7 } from "./core-api/ids";
import { i18n } from "./locales";
import { fetchPluginTerminalSessionSnapshot, type PluginTerminalSessionSummary } from "./core-api/plugin-terminal";
import { createTerminalPane, type TerminalLayoutNode } from "./components/terminal/terminalLayout";
import {
  parseTerminalWorkspaceLayout,
  type PersistedTerminalWorkspaceTab,
} from "./components/terminal/terminalWorkspaceLayout";
import { parseTerminalTabHandoff } from "./terminal-workspace-handoff";
import { showWorkspaceTabFailure } from "./workspace-tab-errors";
import { activateWorkspaceTabView, createManagedWorkspaceTab } from "./workspace-tab-view-shell";
import { snapshotWorkspaceTabs, type WorkspaceTabSnapshot } from "./workspace-tab-windows";

export type TerminalRestoreSessionKind = "ssh" | "local" | "telnet" | "plugin";

/** A live Core Session that the restored Pane attaches to instead of starting a new one. */
export interface TerminalRestoreLivePane {
  paneId: string;
  kind: TerminalRestoreSessionKind;
  sessionId: string;
  generation: string;
}

/** Seed for one restored Terminal Tab; the child WebView rebuilds its Panes from it. */
export interface TerminalRestoreSeed {
  behavior: "restore";
  tab: PersistedTerminalWorkspaceTab;
  /** Start fresh Sessions for saved Hosts; only when history is restored without live Sessions. */
  autoReconnect: boolean;
  live: TerminalRestoreLivePane[];
}

export interface TerminalRestorePlan {
  activeTabId: string | null;
  tabs: TerminalRestoreSeed[];
}

interface Identity { kind: TerminalRestoreSessionKind; sessionId: string; generation: string }

export function isActiveSshSession(summary: SshSessionSummary): boolean {
  return !["closed", "failed"].includes(summary.state);
}

export function isActiveLocalSession(summary: LocalSessionSummary): boolean {
  return !["exited", "closed"].includes(summary.state)
    && (summary.state !== "failed" || summary.failureReason?.code === "processCleanupFailed");
}

export function isActiveTelnetSession(summary: TelnetSessionSummary): boolean {
  return !["closed", "failed"].includes(summary.state);
}

export function isActivePluginSession(summary: PluginTerminalSessionSummary): boolean {
  return summary.cleanupBlocked || !["closed", "failed"].includes(summary.state);
}

function registeredIdentities(registry: WorkspaceTabSnapshot): { claimed: Identity[]; uncertain: boolean } {
  const owned = registry.owned.filter((tab) => tab.kind === "terminal").map((tab) =>
    (parseTerminalTabHandoff(tab.payload)?.panes ?? []).flatMap((pane): Identity[] =>
      pane.kind === "ssh" || pane.kind === "local" || pane.kind === "telnet" || pane.kind === "plugin"
        ? [{ kind: pane.kind, sessionId: pane.sessionId, generation: pane.generation }] : []));
  const others = registry.others.filter((tab) => tab.kind === "terminal").map((tab) =>
    tab.terminalPanes.flatMap((pane): Identity[] => pane.sessionId && pane.generation
      && ["ssh", "local", "telnet", "plugin"].includes(pane.kind)
      ? [{ kind: pane.kind as TerminalRestoreSessionKind, sessionId: pane.sessionId, generation: pane.generation }] : []));
  // A registered Tab without an identified Pane may still own a Session whose
  // projection has not been written; only Sessions bound to a persisted Pane are safe then.
  return { claimed: [...owned, ...others].flat(), uncertain: [...owned, ...others].some((panes) => !panes.length) };
}

/**
 * Plans startup restoration from SQLite layout and Core Sessions. Tabs that
 * Core already registers keep their views; live Sessions always take priority
 * over restarting a Pane, and history restarts Sessions only when none are live.
 */
export async function planTerminalWorkspaceRestore(restoreHistory: boolean): Promise<TerminalRestorePlan> {
  const [ssh, local, telnet, stored, plugin, registry] = await Promise.all([
    fetchSshSessionSnapshot(), fetchLocalSessionSnapshot(), fetchTelnetSessionSnapshot(),
    fetchTerminalWorkspaceLayout(), fetchPluginTerminalSessionSnapshot(), snapshotWorkspaceTabs(),
  ]);
  const layout = parseTerminalWorkspaceLayout(stored.layout);
  const registeredIds = new Set([...registry.owned, ...registry.others]
    .filter((tab) => tab.kind === "terminal").map((tab) => tab.id));
  const { claimed, uncertain } = registeredIdentities(registry);
  const persistedTabs = layout?.tabs ?? [];
  const registeredPaneIds = new Set(persistedTabs.filter((tab) => registeredIds.has(tab.tabId))
    .flatMap((tab) => tab.panes.map((pane) => pane.paneId)));
  const candidates = persistedTabs.filter((tab) => !registeredIds.has(tab.tabId));
  const tabOfPane = new Map(candidates.flatMap((tab) => tab.panes.map((pane) => [pane.paneId, tab.tabId] as const)));
  const isClaimed = (identity: Identity) => claimed.some((item) => item.kind === identity.kind
    && item.sessionId === identity.sessionId && item.generation === identity.generation);

  const live = new Map<string, TerminalRestoreLivePane[]>();
  const synthetic: TerminalRestoreSeed[] = [];
  const assign = (tabId: string, pane: TerminalRestoreLivePane) => live.set(tabId, [...live.get(tabId) ?? [], pane]);
  const adoptOrphans = (panes: TerminalRestoreLivePane[], tabId = createUuidV7()) => {
    // Placeholders that the live Sessions' Panes replace during restore, side by side.
    const layout = panes.slice(1).reduce<TerminalLayoutNode>((first, pane, index) => ({
      kind: "split", splitId: createUuidV7(), direction: "horizontal", ratio: (index + 1) / (index + 2),
      first, second: createTerminalPane(pane.paneId, pane.paneId),
    }), createTerminalPane(panes[0]!.paneId, panes[0]!.paneId));
    synthetic.push({ behavior: "restore", autoReconnect: false, live: panes, tab: {
      tabId, layout, activePaneId: panes[0]!.paneId,
      panes: panes.map((pane) => ({ kind: "launcher", paneId: pane.paneId, label: i18n.global.t("navigation.terminal") })),
    } });
  };
  const adoptOrphan = (pane: TerminalRestoreLivePane) => adoptOrphans([pane]);

  const attached = await Promise.all([
    ...ssh.sessions.filter(isActiveSshSession).map(async (summary) => ({ kind: "ssh" as const,
      summary, attachments: (await getSshSession(summary.sessionId)).attachments })),
    ...local.sessions.filter(isActiveLocalSession).map(async (summary) => ({ kind: "local" as const,
      summary, attachments: (await getLocalSession(summary.sessionId)).attachments })),
  ]);
  for (const { kind, summary, attachments } of attached) {
    const identity = { kind, sessionId: summary.sessionId, generation: summary.generation };
    if (isClaimed(identity) || attachments.some((item) => registeredPaneIds.has(item.viewId))) continue;
    const paneId = attachments.map((item) => item.viewId).find((viewId) => tabOfPane.has(viewId));
    if (paneId) assign(tabOfPane.get(paneId)!, { ...identity, paneId });
    else if (!uncertain) adoptOrphan({ ...identity, paneId: createUuidV7() });
  }
  const unusedTelnetPanes = new Set(candidates.flatMap((tab) => tab.panes)
    .filter((pane) => pane.kind === "telnet").map((pane) => pane.paneId));
  for (const summary of telnet.sessions.filter(isActiveTelnetSession)) {
    const identity = { kind: "telnet" as const, sessionId: summary.sessionId, generation: summary.generation };
    if (isClaimed(identity)) continue;
    // Telnet has no view attachment identity; match the persisted endpoint once.
    const match = candidates.flatMap((tab) => tab.panes).find((pane) => pane.kind === "telnet"
      && unusedTelnetPanes.has(pane.paneId) && pane.address === summary.endpoint.address && pane.port === summary.endpoint.port);
    if (match) {
      unusedTelnetPanes.delete(match.paneId);
      assign(tabOfPane.get(match.paneId)!, { ...identity, paneId: match.paneId });
    } else if (!uncertain) adoptOrphan({ ...identity, paneId: createUuidV7() });
  }
  // Core binds a plugin Session to its Tab and Pane; it can only return to that identity.
  const pluginOrphans = new Map<string, TerminalRestoreLivePane[]>();
  for (const summary of plugin.sessions.filter(isActivePluginSession)) {
    if (registeredIds.has(summary.tabId)) continue;
    const identity = { kind: "plugin" as const, sessionId: summary.sessionId, generation: summary.generation, paneId: summary.paneId };
    if (tabOfPane.get(summary.paneId) === summary.tabId) assign(summary.tabId, identity);
    else if (!uncertain && !persistedTabs.some((tab) => tab.tabId === summary.tabId)) {
      pluginOrphans.set(summary.tabId, [...pluginOrphans.get(summary.tabId) ?? [], identity]);
    }
  }
  for (const [tabId, panes] of pluginOrphans) adoptOrphans(panes, tabId);

  const hasLiveSessions = live.size > 0 || synthetic.length > 0;
  const restored = hasLiveSessions || restoreHistory ? candidates.map((tab): TerminalRestoreSeed => ({
    behavior: "restore", tab, autoReconnect: !hasLiveSessions && restoreHistory, live: live.get(tab.tabId) ?? [],
  })) : [];
  const tabs = [...restored, ...synthetic];
  const activeTabId = tabs.some((seed) => seed.tab.tabId === layout?.activeTabId)
    ? layout!.activeTabId : tabs[0]?.tab.tabId ?? null;
  return { activeTabId, tabs };
}

let restoration: Promise<void> | null = null;

/** Restores the Terminal workspace once per main renderer; each Tab gets its own native WebView. */
export function restoreTerminalWorkspace(restoreHistory: boolean): Promise<void> {
  restoration ??= (async () => {
    let plan: TerminalRestorePlan;
    try { plan = await planTerminalWorkspaceRestore(restoreHistory); }
    catch (error) {
      showWorkspaceTabFailure(error, "workspace-tab-restore", "workspace_tab.restoration_failed");
      return;
    }
    for (const seed of plan.tabs) {
      try {
        // A crashed parent must not replay a restore that may already have started Sessions.
        const active = seed.tab.panes.find((pane) => pane.paneId === seed.tab.activePaneId);
        await createManagedWorkspaceTab(seed.tab.tabId, "terminal", "/terminal", seed,
          { pendingKind: "terminalRestore" }, active ? { label: active.label, stateLabel: "" } : null);
      } catch (error) {
        showWorkspaceTabFailure(error, `workspace-tab-restore:${seed.tab.tabId}`, "workspace_tab.restoration_failed");
      }
    }
    if (plan.activeTabId && plan.activeTabId !== plan.tabs.at(-1)?.tab.tabId) {
      await activateWorkspaceTabView(plan.activeTabId).catch(() => undefined);
    }
  })();
  return restoration;
}

