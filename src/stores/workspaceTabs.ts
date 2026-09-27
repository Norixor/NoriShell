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

export type WorkspacePageType = "newPage" | "knownHosts" | "sshIdentities" | "plugin";
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
}
export interface DesktopHeaderController {
  activate(tabId: string): void;
  close(tabId: string): void;
  closeMany(tabIds: readonly string[]): void;
  deactivate(): void;
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

export const useWorkspaceTabsStore = defineStore("workspaceTabs", () => {
  const terminalTabs = ref<TerminalHeaderTabSnapshot[]>([]);
  const activeTerminalTabId = ref("");
  const terminalBusy = ref(false);
  const terminalActivationBlocked = ref(false);
  const quickCommandsOpen = ref(false);
  const pageTabs = ref<WorkspacePageTab[]>([]);
  const terminalController = shallowRef<TerminalHeaderController | null>(null);
  const createTerminalPending = ref<"default" | "local" | null>(null);
  const desktopTabs = ref<DesktopHeaderTabSnapshot[]>([]);
  const activeDesktopTabId = ref("");
  const desktopBusy = ref(false);
  const desktopController = shallowRef<DesktopHeaderController | null>(null);
  const fileTabs = ref<FileHeaderTab[]>([]);
  const activeFileTabId = ref("");
  const fileControllers = shallowRef(new Map<string, FileHeaderController>());
  const fileSessionOwners = shallowRef(new Map<string, string>());
  const consumedFileFocusOperations = new Set<string>();

  function createFileTab(kind: FileHeaderTab["kind"], hostId: string | null = null, label = "") {
    const groupId = `file:${crypto.randomUUID()}`;
    fileTabs.value.push({ groupId, kind, hostId, label, paneCount: kind === "local" ? 1 : 2 });
    activeFileTabId.value = groupId;
    return groupId;
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

  function requestCloseFileTab(groupId: string) {
    return fileControllers.value.get(groupId)?.requestClose() ?? Promise.resolve(false);
  }

  function fileSessionOwner(sessionId: string) {
    return fileSessionOwners.value.get(sessionId) ?? null;
  }

  function consumeFileFocusOperation(operationId: string) {
    if (consumedFileFocusOperations.has(operationId)) return false;
    consumedFileFocusOperations.add(operationId);
    return true;
  }

  function claimFileSession(sessionId: string, groupId: string) {
    if (!fileTabs.value.some((tab) => tab.groupId === groupId)) return false;
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
      const pending = createTerminalPending.value;
      createTerminalPending.value = null;
      if (pending === "local") controller.createLocal();
      else controller.create();
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

  function queueTerminalCreation(kind: "default" | "local" = "default") {
    createTerminalPending.value = kind;
  }

  return {
    fileTabs,
    activeFileTabId,
    createFileTab,
    syncFilePaneCount,
    activateFileTab,
    showFileWelcome,
    registerFileController,
    requestCloseFileTab,
    fileSessionOwner,
    consumeFileFocusOperation,
    claimFileSession,
    releaseFileSession,
    finishCloseFileTab,
    runFileShortcut,
    desktopTabs,
    activeDesktopTabId,
    desktopBusy,
    desktopController,
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
    queueTerminalCreation,
  };
});
