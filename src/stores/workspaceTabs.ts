import { defineStore } from "pinia";
import { ref, shallowRef } from "vue";

import type {
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
import type { FileTabHandoffSnapshot } from "../views/fileTabHandoffSnapshot";

export type WorkspacePageType = "knownHosts" | "sshIdentities" | "plugin";
type BuiltinWorkspacePageType = Exclude<WorkspacePageType, "plugin">;

export interface TerminalHeaderTabSnapshot {
  groupId: string;
  label: string;
  stateLabel: string;
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

export interface TerminalHeaderController {
  /** Read and validate only layout plus live Session identities; does not freeze the source. */
  snapshotTabHandoff?(tabId: string): Promise<TerminalTabHandoffSnapshot>;
  /** Observe safe per-Tab recovery snapshots for the current Core owner registry. */
  observeTabHandoffSnapshots?(listener: (snapshots: readonly TerminalTabHandoffSnapshot[]) => void): () => void;
  /** Freeze the previously snapshotted Tab after Core has prepared a transfer ticket. */
  freezeTabHandoff?(tabId: string): Promise<void>;
  /** Rebind the exact existing Sessions in this WebView; never restore the global SQLite layout. */
  importTabHandoff?(snapshot: TerminalTabHandoffSnapshot): Promise<void>;
  /** Admit an already imported Tab to this window's CAS persistence ownership after Core commits. */
  commitImportedTabHandoff?(tabId: string): Promise<void>;
  /** Remove a frozen source Tab after the owner transfer commits. */
  commitTabHandoff?(tabId: string): void;
  /** Restore a frozen source Tab when owner transfer or target import fails. */
  rollbackTabHandoff?(tabId: string): Promise<void>;
  /** Remove an imported target Tab when owner transfer fails. */
  discardImportedTab?(tabId: string): Promise<void>;
  /** Activate only an existing exactly matching SSH or local Pane; return false when blocked or missing. */
  /** A failed resource may have no Channel; focus only the existing SSH session and generation. */
  focusNativeSession(scope: NativeTerminalSessionScope): boolean;
  focusSshSession(sessionId: SshSessionId, generation: WireSequence): boolean;
  /** A failed resource may have no PTY; focus only the existing local session and generation. */
  focusLocalSession(sessionId: LocalSessionId, generation: WireSequence): boolean;
  activate(tabId: string): void;
  close(tabId: string): void;
  closeMany(tabIds: readonly string[]): void;
  create(): void;
  /** Controlled entry point: return false while a blocking dialog is open; never create and discard a default PTY. */
  createLocal(): boolean;
  quickConnect(): boolean;
  deactivate(): void;
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
}
export interface FileHeaderController {
  requestClose(): Promise<boolean>;
  runShortcut(commandId: ShortcutCommandId): void;
  snapshotHandoff(): FileTabHandoffSnapshot;
  freezeHandoff(): void;
  rollbackHandoff(): void;
  commitHandoff(): void;
  observeHandoffSnapshot(listener: (snapshot: FileTabHandoffSnapshot) => void): () => void;
}
export interface DesktopHeaderController {
  activate(tabId: string): void;
  close(tabId: string): void;
  closeMany(tabIds: readonly string[]): void;
  deactivate(): void;
  snapshotHandoff?(tabId: string): import("../workspace-desktop-handoff").DesktopTabHandoffSnapshot;
  freezeHandoff?(tabId: string): Promise<void>;
  importHandoff?(session: import("../core-api/generated/core-api").DesktopSessionSummary): Promise<void>;
  commitHandoff?(tabId: string): void;
  rollbackHandoff?(tabId: string): Promise<void>;
  discardHandoff?(tabId: string): Promise<void>;
  admitHandoff?(tabId: string): Promise<void>;
}

const PAGE_DEFINITIONS: Readonly<Record<BuiltinWorkspacePageType, Omit<WorkspacePageTab, "groupId">>> = {
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

export const useWorkspaceTabsStore = defineStore("workspaceTabs", () => {
  const terminalTabs = ref<TerminalHeaderTabSnapshot[]>([]);
  const activeTerminalTabId = ref("");
  const terminalBusy = ref(false);
  const terminalActivationBlocked = ref(false);
  const quickCommandsOpen = ref(false);
  const pageTabs = ref<WorkspacePageTab[]>([]);
  const terminalController = shallowRef<TerminalHeaderController | null>(null);
  const createTerminalPending = ref(false);
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

  function createFileTab(kind: FileHeaderTab["kind"], hostId: string | null = null, label = "") {
    const groupId = `file:${crypto.randomUUID()}`;
    fileTabs.value.push({ groupId, kind, hostId, label, paneCount: kind === "local" ? 1 : 2 });
    activeFileTabId.value = groupId;
    return groupId;
  }

  function importFileTab(tab: FileHeaderTab) {
    if (!/^file:[0-9a-f-]{36}$/i.test(tab.groupId) || (tab.kind !== "local" && tab.kind !== "remote")
      || (tab.hostId !== null && typeof tab.hostId !== "string") || typeof tab.label !== "string"
      || !Number.isSafeInteger(tab.paneCount) || tab.paneCount < 1) return false;
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

  function showFileWelcome() { activeFileTabId.value = ""; }

  function registerFileController(groupId: string, controller: FileHeaderController) {
    fileControllers.value = new Map(fileControllers.value).set(groupId, controller);
    return () => {
      if (fileControllers.value.get(groupId) !== controller) return;
      const next = new Map(fileControllers.value);
      next.delete(groupId);
      fileControllers.value = next;
    };
  }

  function requestCloseFileTab(groupId: string) {
    return fileControllers.value.get(groupId)?.requestClose() ?? Promise.resolve(false);
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
    if (createTerminalPending.value) {
      createTerminalPending.value = false;
      controller.create();
    }
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

  function ensurePageTabForRoute(route: string) {
    const definition = Object.values(PAGE_DEFINITIONS).find((page) => page.route === route);
    if (!definition) return null;
    const groupId = `page:${definition.pageType}`;
    if (!pageTabs.value.some((tab) => tab.groupId === groupId)) {
      pageTabs.value.push({ groupId, ...definition });
    }
    return groupId;
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
      if (tab.groupId !== `page:${tab.pageType}` || tab.route !== builtin.route) return false;
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
    const groupId = `page:plugin:${item.pluginId}:${item.navigation.pageId}`;
    const route = `/plugin/${encodeURIComponent(item.pluginId)}/${encodeURIComponent(item.navigation.pageId)}`;
    const existing = pageTabs.value.find((tab) => tab.groupId === groupId);
    if (existing) {
      existing.label = item.navigation.label;
      existing.iconName = item.navigation.icon;
      existing.route = route;
      return groupId;
    }
    pageTabs.value.push({
      groupId,
      pageType: "plugin",
      route,
      labelKey: null,
      label: item.navigation.label,
      iconName: item.navigation.icon,
    });
    return groupId;
  }

  function closePluginPageTabs(pluginId: string) {
    pageTabs.value = pageTabs.value.filter((tab) => (
      tab.pageType !== "plugin" || !tab.groupId.startsWith(`page:plugin:${pluginId}:`)
    ));
  }

  function queueTerminalCreation() {
    createTerminalPending.value = true;
  }

  return {
    desktopTabs,
    activeDesktopTabId,
    desktopBusy,
    desktopController,
    fileTabs,
    activeFileTabId,
    createFileTab,
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
    ensurePageTabForRoute,
    ensurePluginPageTab,
    closePluginPageTabs,
    closePageTab,
    importPageTab,
    queueTerminalCreation,
  };
});
