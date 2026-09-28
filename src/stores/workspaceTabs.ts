import { defineStore } from "pinia";
import { ref, shallowRef } from "vue";

import type {
  PluginApprovedTerminalChannelLaunch,
  NativeTerminalSessionScope,
  LocalSessionId,
  PluginNavigationItem,
  SshSessionId,
  TelnetSessionId,
  TelnetSocketId,
  WireSequence,
} from "../core-api/generated/core-api";
import type { ShortcutCommandId } from "../shortcuts";
import type { TerminalTabHandoffSnapshot } from "../terminal-workspace-handoff";
import type { TerminalRestoreSeed } from "../workspace-tab-terminal-restore";
import type { FileTabHandoffSnapshot } from "../views/fileTabHandoffSnapshot";

export type WorkspacePageType = "newPage" | "knownHosts" | "sshIdentities" | "plugin";
type BuiltinWorkspacePageType = Exclude<WorkspacePageType, "plugin">;

export interface TerminalHeaderTabSnapshot {
  groupId: string;
  label: string;
  stateLabel: string;
  paneCount?: number;
  hostId?: string | null;
  completionCount?: number;
  bellAttention?: boolean;
}

export interface WorkspacePageTab {
  groupId: string;
  pageType: WorkspacePageType;
  route: string;
  labelKey: string | null;
  label: string;
  iconName: string | null;
}

/** A saved Host to open in the Terminal: from a route query or a new Tab's bootstrap intent. */
export interface TerminalHostOpenRequest {
  hostId: string;
  /** Consumed once; a repeated id never opens a second connection. */
  operationId?: string;
  source?: string;
  pluginAuthorizationToken?: string;
}

export interface TerminalHeaderController {
  /** Create this child WebView's first Tab with the id reserved by the native manager. */
  createInitialTab?(id: string, behavior: "welcome" | "local", initialLocalOpen?: {
    paneId: string;
    openAttemptId: string;
    operationId: string;
    attachAttemptId: string;
    initialRows: number;
    initialCols: number;
  }): Promise<void>;
  createInitialPluginProtocolTab?(id: string, request: { launchId: string; revision: string; paneId: string }): Promise<void>;
  createInitialApprovedPluginChannel?(id: string, payload: PluginApprovedTerminalChannelLaunch): Promise<void>;
  /** Restore this child WebView's first Tab from a startup seed planned by the main shell. */
  restoreInitialTab?(id: string, seed: TerminalRestoreSeed): Promise<void>;
  /** Read the Tab's bounded recovery projection: layout plus live Session identities. */
  snapshotTabHandoff?(tabId: string): Promise<TerminalTabHandoffSnapshot>;
  /** Observe safe per-Tab recovery snapshots for the current Core owner registry. */
  observeTabHandoffSnapshots?(listener: (snapshots: readonly TerminalTabHandoffSnapshot[]) => void): () => void;
  /** Crash recovery: rebind the exact existing Sessions in this WebView. */
  importTabHandoff?(snapshot: TerminalTabHandoffSnapshot): Promise<void>;
  /** Admit an imported Tab to this renderer's persistence ownership. */
  commitImportedTabHandoff?(tabId: string): Promise<void>;
  /** Remove an imported Tab whose recovery failed. */
  discardImportedTab?(tabId: string): Promise<void>;
  /** Activate only an existing exactly matching SSH or local Pane; return false when blocked or missing. */
  /** A failed resource may have no Channel; focus only the existing SSH session and generation. */
  focusNativeSession(scope: NativeTerminalSessionScope): boolean;
  focusSshSession(sessionId: SshSessionId, generation: WireSequence): boolean;
  /** A failed resource may have no PTY; focus only the existing local session and generation. */
  focusLocalSession(sessionId: LocalSessionId, generation: WireSequence): boolean;
  activate(tabId: string): void;
  close(tabId: string): boolean;
  closeMany(tabIds: readonly string[], confirmed?: boolean): boolean;
  quickConnect(target?: string): boolean;
  openTelnet(): boolean;
  /** Resolves once the Pane shows the Host's connecting or interactive step, not after it connects. */
  openHost?(request: TerminalHostOpenRequest): Promise<void>;
  deactivate(): Promise<boolean> | void;
  toggleQuickCommands(): void;
  runShortcut?(commandId: ShortcutCommandId): void;
  /** Activate only an existing Telnet Pane with exactly matching session, generation, and socket; never reconnect. */
  focusTelnetSession(sessionId: TelnetSessionId, generation: WireSequence, socketId: TelnetSocketId | null): boolean;
}

export interface DesktopHeaderTabSnapshot {
  groupId: string;
  label: string;
  stateLabel: string;
}
export interface FileHeaderTab {
  groupId: string;
  kind: "local" | "remote";
  hostId: string | null;
  label: string;
  paneCount: number;
  initialSessionId?: string | null;
  initialGeneration?: string | null;
}
export interface FileHeaderController {
  requestClose(confirmed?: boolean): Promise<boolean>;
  runShortcut(commandId: ShortcutCommandId): void;
  snapshotHandoff(): FileTabHandoffSnapshot;
  observeHandoffSnapshot(listener: (snapshot: FileTabHandoffSnapshot) => void): () => void;
}
export interface DesktopHeaderController {
  activate(tabId: string): void;
  beginOpenProfile(profileId: string): Promise<void>;
  close(tabId: string): Promise<void>;
  deactivate(): void;
  isBusy(): boolean;
  snapshotHandoff?(tabId: string): import("../workspace-desktop-handoff").DesktopTabHandoffSnapshot;
  importIdle?(tabId: string, profileId: string): void | Promise<void>;
  importHandoff?(session: import("../core-api/generated/core-api").DesktopSessionSummary): Promise<void>;
  discardHandoff?(tabId: string): Promise<void>;
  admitHandoff?(tabId: string): Promise<void>;
}

const PAGE_DEFINITIONS: Readonly<Record<BuiltinWorkspacePageType, Omit<WorkspacePageTab, "groupId">>> = {
  newPage: {
    pageType: "newPage",
    route: "/new",
    labelKey: "newWorkspace.title",
    label: "",
    iconName: null,
  },
  knownHosts: {
    pageType: "knownHosts",
    route: "/known-hosts",
    labelKey: "sshSettings.hostKeys.title",
    label: "",
    iconName: null,
  },
  sshIdentities: {
    pageType: "sshIdentities",
    route: "/settings/identities",
    labelKey: "identitySettings.title",
    label: "",
    iconName: null,
  },
};

export function workspacePageTabForRoute(route: string): WorkspacePageTab | null {
  const definition = Object.values(PAGE_DEFINITIONS).find((page) => page.route === route);
  return definition ? { groupId: `page:${definition.pageType}`, ...definition } : null;
}

/** The Page Tab identity and route of one plugin navigation page. */
export function pluginPageTab(item: PluginNavigationItem): WorkspacePageTab {
  return {
    groupId: `page:plugin:${item.pluginId}:${item.navigation.pageId}`,
    pageType: "plugin",
    route: `/plugin/${encodeURIComponent(item.pluginId)}/${encodeURIComponent(item.navigation.pageId)}`,
    labelKey: null,
    label: item.navigation.label,
    iconName: item.navigation.icon,
  };
}

export const useWorkspaceTabsStore = defineStore("workspaceTabs", () => {
  const terminalTabs = ref<TerminalHeaderTabSnapshot[]>([]);
  const activeTerminalTabId = ref("");
  const terminalBusy = ref(false);
  const terminalActivationBlocked = ref(false);
  const quickCommandsOpen = ref(false);
  const pageTabs = ref<WorkspacePageTab[]>([]);
  const terminalController = shallowRef<TerminalHeaderController | null>(null);
  const desktopTabs = ref<DesktopHeaderTabSnapshot[]>([]);
  const activeDesktopTabId = ref("");
  const desktopBusy = ref(false);
  const desktopController = shallowRef<DesktopHeaderController | null>(null);
  const fileTabs = ref<FileHeaderTab[]>([]);
  const activeFileTabId = ref("");
  const fileControllers = shallowRef(new Map<string, FileHeaderController>());
  const fileSessionOwners = shallowRef(new Map<string, string>());
  const importedFileSnapshots = shallowRef(new Map<string, FileTabHandoffSnapshot>());
  const consumedFileFocusOperations = new Set<string>();

  function createInitialFileTab(id: string, kind: FileHeaderTab["kind"], hostId: string | null, label: string,
    initialSessionId?: string | null, initialGeneration?: string | null): boolean {
    if (!/^file:[0-9a-f-]{36}$/i.test(id) || fileTabs.value.some((tab) => tab.groupId === id)
      || (kind !== "local" && kind !== "remote") || (hostId !== null && typeof hostId !== "string")
      || (initialSessionId != null && (kind !== "remote" || (!hostId && !initialGeneration)
        || !/^[0-9a-f-]{36}$/i.test(initialSessionId)))
      || (initialGeneration != null && (!initialSessionId || !/^[0-9]{1,20}$/u.test(initialGeneration)))) return false;
    fileTabs.value.push({ groupId: id, kind, hostId, label, paneCount: kind === "local" ? 1 : 2,
      ...(initialSessionId ? { initialSessionId } : {}), ...(initialGeneration ? { initialGeneration } : {}) });
    activeFileTabId.value = id;
    return true;
  }

  function importFileTab(tab: FileHeaderTab) {
    if (!/^file:[0-9a-f-]{36}$/i.test(tab.groupId) || (tab.kind !== "local" && tab.kind !== "remote")
      || (tab.hostId !== null && typeof tab.hostId !== "string") || typeof tab.label !== "string"
      || !Number.isSafeInteger(tab.paneCount) || tab.paneCount < 1
      || (tab.initialSessionId != null && (tab.kind !== "remote" || (!tab.hostId && !tab.initialGeneration)
        || !/^[0-9a-f-]{36}$/i.test(tab.initialSessionId)))
      || (tab.initialGeneration != null && (!tab.initialSessionId || !/^[0-9]{1,20}$/u.test(tab.initialGeneration)))) return false;
    if (fileTabs.value.some((existing) => existing.groupId === tab.groupId)) return false;
    fileTabs.value.push({ ...tab });
    return true;
  }
  function syncFilePaneCount(groupId: string, paneCount: number) {
    const tab = fileTabs.value.find((item) => item.groupId === groupId);
    if (tab && tab.paneCount !== paneCount) tab.paneCount = paneCount;
  }

  function activateFileTab(groupId: string) {
    if (!fileTabs.value.some((tab) => tab.groupId === groupId)) return false;
    activeFileTabId.value = groupId;
    return true;
  }

  function showFileWelcome() {
    activeFileTabId.value = "";
  }

  function registerFileController(groupId: string, controller: FileHeaderController) {
    fileControllers.value = new Map(fileControllers.value).set(groupId, controller);
    return () => {
      if (fileControllers.value.get(groupId) !== controller) return;
      const next = new Map(fileControllers.value);
      next.delete(groupId);
      fileControllers.value = next;
    };
  }

  function requestCloseFileTab(groupId: string, confirmed = false) {
    return fileControllers.value.get(groupId)?.requestClose(confirmed) ?? Promise.resolve(false);
  }

  function fileController(groupId: string) { return fileControllers.value.get(groupId) ?? null; }

  function stageImportedFileTab(snapshot: FileTabHandoffSnapshot) {
    if (fileTabs.value.some((tab) => tab.groupId === snapshot.tab.groupId)
      || importedFileSnapshots.value.has(snapshot.tab.groupId)) return false;
    const sessions = snapshot.panes.flatMap((pane) => pane.endpoint.kind === "remote" && pane.endpoint.sessionId
      ? [pane.endpoint.sessionId] : []);
    if (sessions.some((sessionId) => fileSessionOwners.value.has(sessionId)
      || [...importedFileSnapshots.value.values()].some((staged) => staged.panes.some((pane) =>
        pane.endpoint.kind === "remote" && pane.endpoint.sessionId === sessionId)))) return false;
    importedFileSnapshots.value = new Map(importedFileSnapshots.value).set(snapshot.tab.groupId, snapshot);
    return true;
  }

  function admitImportedFileTab(groupId: string) {
    const snapshot = importedFileSnapshots.value.get(groupId);
    if (!snapshot) return false;
    const sessions = snapshot.panes.flatMap((pane) => pane.endpoint.kind === "remote" && pane.endpoint.sessionId
      ? [pane.endpoint.sessionId] : []);
    if (sessions.some((sessionId) => fileSessionOwners.value.get(sessionId)
      && fileSessionOwners.value.get(sessionId) !== groupId)) return false;
    if (!fileTabs.value.some((tab) => tab.groupId === groupId) && !importFileTab(snapshot.tab)) return false;
    const owners = new Map(fileSessionOwners.value);
    for (const sessionId of sessions) owners.set(sessionId, groupId);
    fileSessionOwners.value = owners;
    activeFileTabId.value = groupId;
    return true;
  }

  function discardImportedFileTab(groupId: string) {
    const next = new Map(importedFileSnapshots.value);
    next.delete(groupId);
    importedFileSnapshots.value = next;
    finishCloseFileTab(groupId);
  }

  function fileSessionOwner(sessionId: string) { return fileSessionOwners.value.get(sessionId) ?? null; }
  function consumeFileFocusOperation(operationId: string) {
    if (consumedFileFocusOperations.has(operationId)) return false;
    consumedFileFocusOperations.add(operationId);
    return true;
  }

  function claimFileSession(sessionId: string, groupId: string) {
    if (!fileTabs.value.some((tab) => tab.groupId === groupId)) return false;
    if ([...importedFileSnapshots.value].some(([stagedId, staged]) => stagedId !== groupId
      && staged.panes.some((pane) => pane.endpoint.kind === "remote" && pane.endpoint.sessionId === sessionId))) return false;
    const owner = fileSessionOwners.value.get(sessionId);
    if (owner && owner !== groupId) return false;
    fileSessionOwners.value = new Map(fileSessionOwners.value).set(sessionId, groupId);
    return true;
  }

  function releaseFileSession(sessionId: string, groupId: string) {
    if (fileSessionOwners.value.get(sessionId) !== groupId) return;
    const next = new Map(fileSessionOwners.value);
    next.delete(sessionId);
    fileSessionOwners.value = next;
  }

  function finishCloseFileTab(groupId: string) {
    const index = fileTabs.value.findIndex((tab) => tab.groupId === groupId);
    const imported = new Map(importedFileSnapshots.value);
    imported.delete(groupId);
    importedFileSnapshots.value = imported;
    if (index < 0) return;
    fileTabs.value.splice(index, 1);
    fileSessionOwners.value = new Map([...fileSessionOwners.value].filter(([, owner]) => owner !== groupId));
    if (activeFileTabId.value === groupId) {
      activeFileTabId.value = fileTabs.value[Math.min(index, fileTabs.value.length - 1)]?.groupId ?? "";
    }
  }

  function runFileShortcut(commandId: ShortcutCommandId) {
    fileControllers.value.get(activeFileTabId.value)?.runShortcut(commandId);
  }

  function syncDesktopState(input: { tabs: readonly DesktopHeaderTabSnapshot[]; activeTabId: string; busy: boolean }) {
    desktopTabs.value = input.tabs.map((tab) => ({ ...tab }));
    activeDesktopTabId.value = input.activeTabId;
    desktopBusy.value = input.busy;
  }

  function registerDesktopController(controller: DesktopHeaderController) {
    desktopController.value = controller;
    return () => {
      if (desktopController.value !== controller) return;
      desktopController.value = null;
      desktopTabs.value = [];
      activeDesktopTabId.value = "";
    };
  }

  function syncTerminalState(input: {
    tabs: readonly TerminalHeaderTabSnapshot[];
    activeTabId: string;
    busy: boolean;
    activationBlocked: boolean;
    quickCommandsOpen: boolean;
  }) {
    terminalTabs.value = input.tabs.map((tab) => ({ ...tab }));
    activeTerminalTabId.value = input.activeTabId;
    terminalBusy.value = input.busy;
    terminalActivationBlocked.value = input.activationBlocked;
    quickCommandsOpen.value = input.quickCommandsOpen;
  }

  function registerTerminalController(controller: TerminalHeaderController) {
    terminalController.value = controller;
    return () => {
      if (terminalController.value !== controller) return;
      terminalController.value = null;
      terminalTabs.value = [];
      activeTerminalTabId.value = "";
      terminalBusy.value = false;
      terminalActivationBlocked.value = false;
      quickCommandsOpen.value = false;
    };
  }

  function closePageTab(groupId: string) {
    const index = pageTabs.value.findIndex((tab) => tab.groupId === groupId);
    if (index < 0) return null;
    const [removed] = pageTabs.value.splice(index, 1);
    return removed ?? null;
  }

  function importPageTab(tab: WorkspacePageTab) {
    const builtin = tab.pageType !== "plugin" ? PAGE_DEFINITIONS[tab.pageType] : null;
    if (builtin) {
      const expectedId = `page:${tab.pageType}`;
      const managedNewPage = tab.pageType === "newPage"
        && /^page:newPage:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(tab.groupId);
      if ((tab.groupId !== expectedId && !managedNewPage) || tab.route !== builtin.route) return false;
    } else if (tab.pageType === "plugin") {
      const route = /^\/plugin\/([^/]+)\/([^/]+)$/.exec(tab.route);
      if (!route) return false;
      try {
        if (tab.groupId !== `page:plugin:${decodeURIComponent(route[1]!)}:${decodeURIComponent(route[2]!)}`) return false;
      } catch { return false; }
    } else return false;
    if (pageTabs.value.some((existing) => existing.groupId === tab.groupId)) return true;
    pageTabs.value.push({ ...tab });
    return true;
  }

  function ensurePluginPageTab(item: PluginNavigationItem) {
    const tab = pluginPageTab(item);
    const existing = pageTabs.value.find((candidate) => candidate.groupId === tab.groupId);
    if (existing) Object.assign(existing, { label: tab.label, iconName: tab.iconName, route: tab.route });
    else pageTabs.value.push(tab);
    return tab.groupId;
  }

  function closePluginPageTabs(pluginId: string) {
    pageTabs.value = pageTabs.value.filter((tab) => (
      tab.pageType !== "plugin" || !tab.groupId.startsWith(`page:plugin:${pluginId}:`)
    ));
  }

  return {
    desktopTabs,
    activeDesktopTabId,
    desktopBusy,
    desktopController,
    fileTabs,
    activeFileTabId,
    createInitialFileTab,
    importFileTab,
    syncFilePaneCount,
    activateFileTab,
    showFileWelcome,
    registerFileController,
    requestCloseFileTab,
    fileController,
    importedFileSnapshots,
    stageImportedFileTab,
    admitImportedFileTab,
    discardImportedFileTab,
    fileSessionOwner,
    consumeFileFocusOperation,
    claimFileSession,
    releaseFileSession,
    finishCloseFileTab,
    runFileShortcut,
    syncDesktopState,
    registerDesktopController,
    terminalTabs,
    activeTerminalTabId,
    terminalBusy,
    terminalActivationBlocked,
    quickCommandsOpen,
    pageTabs,
    terminalController,
    syncTerminalState,
    registerTerminalController,
    ensurePluginPageTab,
    closePluginPageTabs,
    closePageTab,
    importPageTab,
  };
});
