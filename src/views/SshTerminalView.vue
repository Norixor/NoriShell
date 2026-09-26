<script setup lang="ts">
import { requestSecureCredential } from "../core-api/secure-credential-client";
import { requestSecureVault } from "../core-api/secure-vault-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  computed,
  nextTick,
  onActivated,
  onBeforeUnmount,
  onMounted,
  provide,
  ref,
  watch,
} from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { useRouteReveal } from "../routeReveal";
import { onSavedConnectionsChanged } from "../saved-connections";

import {
  NvxLocalTerminalPane,
  NvxQuickCommandsSidebar,
  NvxSshTerminalPane,
  NvxTelnetTerminalPane,
  NvxTerminalLauncher,
  NvxTerminalPaneControls,
  NvxTerminalSplitTree,
} from "../components/terminal";
import {
  closeTerminalPane,
  countTerminalPanes,
  createTerminalPane,
  findTerminalPane,
  setTerminalForPane,
  setTerminalSplitRatio,
  splitTerminalPane,
  splitTerminalWorkspaceToRight,
  terminalLayoutMinimumSpanAfterSplit,
  terminalLayoutMinimumSpanAfterWorkspaceRightSplit,
  type TerminalLayoutNode,
  type TerminalSplitDirection,
} from "../components/terminal/terminalLayout";
import {
  parseTerminalWorkspaceLayout,
  projectTerminalWorkspaceLayout,
  type PersistedTerminalWorkspaceLayout,
  type PersistedTerminalWorkspacePane,
} from "../components/terminal/terminalWorkspaceLayout";
import {
  NvxButton,
  NvxCheckbox,
  NvxDialog,
  NvxField,
  NvxInlineNotice,
  NvxInput,
  NvxSelect,
} from "../components/ui";
import {
  canUseDesktopCore,
  createHost,
  createIdentity,
  fetchLocalSessionSnapshot,
  fetchSshSessionSnapshot,
  fetchTelnetSessionSnapshot,
  fetchTerminalWorkspaceLayout,
  fetchVaultStatus,
  getHostConnectionConfig,
  getLocalSession,
  getSshSession,
  listHostCatalog,
  listHosts,
  parseCoreApiError,
  replaceAuthenticationPlan,
  replaceTerminalWorkspaceLayout,
  updateHost,
} from "../core-api/client";
import type {
  HostCatalogEntry,
  HostSummary,
  LocalSessionId,
  LocalSessionSnapshot,
  LocalSessionState,
  LocalSessionSummary,
  PluginApprovedTerminalChannelLaunch,
  SshSessionState,
  SshSessionId,
  SshSessionSnapshot,
  SshSessionSummary,
  SshSessionTarget,
  TelnetEndpoint,
  TelnetSessionId,
  TelnetSessionSnapshot,
  TelnetSessionState,
  TelnetSessionSummary,
  TelnetSocketId,
  VaultState,
  WireSequence,
} from "../core-api/generated/core-api";
import NvxPluginTerminalPane from "../components/terminal/NvxPluginTerminalPane.vue";
import {
  fetchPluginTerminalSessionSnapshot, listPluginProtocolLaunches,
  type PluginProtocolLaunchSummary, type PluginTerminalProfile,
  type PluginTerminalSessionSummary, type PluginTerminalSessionState,
} from "../core-api/plugin-terminal";
import { createUuidV7 } from "../core-api/ids";
import { clearTerminalInputFocus } from "../terminal-input-target";
import { registerTerminalWorkspaceFlush } from "../terminal-workspace-persistence";
import { useUiStore } from "../stores/ui";
import type { NativeTerminalSessionScope } from "../core-api/generated/core-api";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import { useTipsStore } from "../stores/tips";
import { applicationPreferenceFailure } from "../core-api/application-preferences";
import type { ShortcutCommandId } from "../shortcuts";
import NvxPluginToolPanel from "../components/plugins/NvxPluginToolPanel.vue";
import NvxPluginFloatingControls from "../components/plugins/NvxPluginFloatingControls.vue";
import NvxTerminalPluginRegion from "../components/terminal/NvxTerminalPluginRegion.vue";
import { terminalPluginToolsKey } from "../components/terminal/terminalPluginTools";
import { takeSftpTerminalLaunch } from "./sftpTerminalLaunch";

interface TerminalLauncherPane {
  kind: "launcher";
  paneId: string;
  label: string;
}

interface SshSessionPane {
  kind: "session";
  paneId: string;
  label: string;
  state: SshSessionState;
  target: SshSessionTarget;
  credentialRefId: string | null;
  pluginAuthorizationToken?: string | null;
  summary: SshSessionSummary | null;
  deferredStart: boolean;
  deferredRecovery: "reconnect" | "credential" | "vaultUnlock";
  initialDirectory?: string | null;
}

interface LocalSessionPane {
  kind: "local";
  paneId: string;
  label: string;
  state: LocalSessionState;
  summary: LocalSessionSummary | null;
  deferredStart: boolean;
}

interface TelnetSessionPane {
  kind: "telnet";
  paneId: string;
  label: string;
  endpoint: TelnetEndpoint;
  state: TelnetSessionState;
  summary: TelnetSessionSummary | null;
  deferredStart: boolean;
}

interface PluginSessionPane {
  kind: "plugin";
  paneId: string;
  label: string;
  profile: PluginTerminalProfile | null;
  launch: PluginProtocolLaunchSummary | null;
  state: PluginTerminalSessionState;
  summary: PluginTerminalSessionSummary | null;
  deferredStart: boolean;
}

type TerminalWorkspacePane = TerminalLauncherPane | SshSessionPane | LocalSessionPane | TelnetSessionPane | PluginSessionPane;

interface TerminalWorkspaceTab {
  tabId: string;
  layout: TerminalLayoutNode;
  activePaneId: string;
  panes: TerminalWorkspacePane[];
}

interface CloseCandidate {
  mode: "pane" | "tab" | "tabs";
  tabIds: string[];
  paneIds: string[];
}

interface OptimisticCloseSnapshot {
  activeTabId: string;
  tabs: Array<{ index: number; tab: TerminalWorkspaceTab }>;
}

interface SshPaneExpose {
  disconnectForClose?(): Promise<void>;
  terminateForClose?(): Promise<void>;
  reconcileAfterForeground?(): Promise<void>;
  reconnectWithCredential?(credentialRefId: string): Promise<void>;
  reconnectSavedCredential?(): Promise<void>;
  activateFromTab(): void;
  deactivateFromTab(): void;
  runShortcut?(commandId: ShortcutCommandId): void;
}

const { t, te } = useI18n();
const route = useRoute();
const router = useRouter();
const ui = useUiStore();
const workspaceTabs = useWorkspaceTabsStore();
const tips = useTipsStore();
const launcherOpen = ref(false);
const telnetLauncherOpen = ref(false);
const telnetAddress = ref("");
const telnetPort = ref("23");
const telnetAcceptsCleartext = ref(false);
const telnetAcceptsMissingIdentity = ref(false);
const telnetAcceptsTampering = ref(false);
const telnetValidationVisible = ref(false);
const address = ref("");
const port = ref("22");
const username = ref("");
const authentication = ref("password");
const saveCredential = ref(false);
const vaultState = ref<VaultState | null>(null);
const vaultStatusLoading = ref(false);
const vaultStatusUnavailable = ref(false);
const vaultDialogOpen = ref(false);
const vaultPromptMode = ref<"create" | "unlock" | null>(null);
const vaultErrorVisible = ref(false);
const vaultPurpose = ref<"saveCredential" | "useSavedCredential">("saveCredential");
const preparing = ref(false);
const validationVisible = ref(false);
const connectionErrorVisible = ref(false);
const connectionErrorKey = ref("sshTerminal.connectFailedTitle");
const connectionErrorBodyKey = ref<string | null>("sshTerminal.connectFailedBody");
const credentialSavedDuringAttempt = ref(false);
const quickCommandsOpen = ref(false);
const tabs = ref<TerminalWorkspaceTab[]>([]);
const activeTabId = ref("");
// BEL attention is renderer-only projection state: it never changes the session or workspace layout.
const bellAttentionPaneIds = ref<Set<string>>(new Set());
const requestedHost = ref<HostSummary | null>(null);
const requestedInitialDirectory = ref<{ hostId: string; paneId: string; directory: string } | null>(null);
const requestedPluginAuthorizationToken = ref<string | null>(null);
const savedHosts = ref<HostSummary[]>([]);
const recentHostEntries = ref<HostCatalogEntry[]>([]);
const closeCandidate = ref<CloseCandidate | null>(null);
const closeConfirmationVisible = ref(false);
const skipFutureSinglePaneTabClosePrompt = ref(false);
const closingTab = ref(false);
const closePreferenceSaving = ref(false);
const closeTabErrorVisible = ref(false);
const workspacePersistenceErrorVisible = ref(false);
const pluginLaunchErrorVisible = ref(false);
const consumingHostOperationId = ref<string | null>(null);
const consumedHostOperationIds = new Set<string>();
const consumingRouteOperationIds = new Set<string>();
const reauthenticationPaneId = ref<string | null>(null);
const pendingVaultReconnectPaneIds = new Set<string>();
const launcherTargetPaneId = ref<string | null>(null);
const paneRefs = new Map<string, SshPaneExpose>();
const CLOSE_TERMINALS_TIMEOUT_MS = 15_000;
let closeTerminalsTimeout: number | null = null;
let optimisticTabCloseInFlight = false;
let optimisticTabCloseAttempt = 0;
let workspaceRevision: string | null = null;

let workspaceInitialized = false;
let workspacePersistenceEnabled = false;
let lastPersistedProjection = "";
let workspaceSaveTimer: number | null = null;
let workspaceInitializationPromise: Promise<void> | null = null;
let workspaceWriteChain: Promise<void> = Promise.resolve();
let unregisterWorkspaceFlush: (() => void) | null = null;
let unregisterTerminalHeaderController: (() => void) | null = null;
let unlistenPluginProtocolLaunch: UnlistenFn | null = null;
let unlistenPluginTerminalChannel: UnlistenFn | null = null;
const approvedPluginChannelOperations = new Set<string>();

const activeTab = computed(() => tabs.value.find((tab) => tab.tabId === activeTabId.value) ?? null);
const activePane = computed(() => activeTab.value?.panes.find(
  (pane) => pane.paneId === activeTab.value?.activePaneId,
) ?? null);
const pluginToolsOpen = ref(false);
const terminalToolCatalogCount = ref(0);
const embeddedSidebarCount = ref(0);
const activePluginContextKey = computed(() => {
  const pane = activePane.value;
  if (pane?.kind === "session") return [
    pane.target.kind === "host" ? pane.target.hostId : "", pane.paneId, "ssh",
    pane.summary?.sessionId ?? "none", pane.summary?.generation ?? "0",
  ].join("|");
  return `|${pane?.paneId ?? "none"}|ssh|none|0`;
});
const activePluginContextLabel = computed(() => activePane.value?.label ?? t("plugins.tools.noSession"));
const activePluginSessionAvailable = computed(() => route.path === "/terminal"
  && activePane.value?.kind === "session" && activePane.value.state === "running"
  && Boolean(activePane.value.summary));
provide(terminalPluginToolsKey, {
  open: (contextKey) => {
    const paneId = contextKey.includes("|") ? contextKey.split("|")[1] : contextKey;
    const tab = tabs.value.find((candidate) => candidate.panes.some((pane) => pane.paneId === paneId));
    if (tab && paneId) { activateTab(tab.tabId); activatePane(tab.tabId, paneId); }
    quickCommandsOpen.value = false;
    pluginToolsOpen.value = true;
  },
  available: computed(() => activePluginSessionAvailable.value && terminalToolCatalogCount.value > 0),
});
watch(activePluginContextKey, () => { terminalToolCatalogCount.value = 0; });
watch(quickCommandsOpen, (open) => { if (open) pluginToolsOpen.value = false; });
const workspaceProjection = computed(() => JSON.stringify(
  projectTerminalWorkspaceLayout(tabs.value, activeTabId.value),
));
const reauthenticating = computed(() => reauthenticationPaneId.value !== null);
const closeCandidateSessionCount = computed(() => closeCandidate.value?.paneIds.length ?? 0);
const closeCandidateCanDisablePrompt = computed(() => {
  const candidate = closeCandidate.value;
  if (!candidate || candidate.mode !== "tab" || candidate.tabIds.length !== 1) return false;
  const tab = tabs.value.find((value) => value.tabId === candidate.tabIds[0]);
  return tab?.panes.length === 1 && candidate.paneIds.length === 1;
});
const launcherHosts = computed(() => {
  if (recentHostEntries.value.length) {
    return recentHostEntries.value.slice(0, 8).map((entry) => ({
      host: entry.host,
      connectedAtUnixMs: entry.recentConnection?.connectedAtUnixMs ?? null,
    }));
  }
  return savedHosts.value.slice(0, 8).map((host) => ({
    host,
    connectedAtUnixMs: null,
  }));
});

const authOptions = computed(() => [
  { value: "password", label: t("sshTerminal.authPassword") },
  { value: "privateKey", label: t("sshTerminal.authPrivateKey") },
]);

function openLauncher() {
  validationVisible.value = false;
  connectionErrorVisible.value = false;
  connectionErrorKey.value = "sshTerminal.connectFailedTitle";
  connectionErrorBodyKey.value = "sshTerminal.connectFailedBody";
  saveCredential.value = false;
  vaultPromptMode.value = null;
  vaultDialogOpen.value = false;
  launcherOpen.value = true;
  void refreshVaultState();
}

function showConnectionError(error: unknown) {
  const coreError = parseCoreApiError(error);
  if (coreError?.messageKey && te(coreError.messageKey)) {
    connectionErrorKey.value = coreError.messageKey;
    connectionErrorBodyKey.value = null;
  } else if (credentialSavedDuringAttempt.value) {
    connectionErrorKey.value = "sshTerminal.savedCredentialConnectFailedTitle";
    connectionErrorBodyKey.value = "sshTerminal.savedCredentialConnectFailedBody";
  } else {
    connectionErrorKey.value = "sshTerminal.connectFailedTitle";
    connectionErrorBodyKey.value = "sshTerminal.connectFailedBody";
  }
  connectionErrorVisible.value = true;
}

function openInlineQuickConnect(target: string, paneId: string | null = null) {
  requestedInitialDirectory.value = null;
  let endpoint = target.trim();
  let parsedUsername = "";
  let parsedAddress = endpoint;
  let parsedPort = "22";
  const atIndex = endpoint.lastIndexOf("@");
  if (atIndex > 0) {
    parsedUsername = endpoint.slice(0, atIndex);
    endpoint = endpoint.slice(atIndex + 1);
  }
  if (endpoint.startsWith("[")) {
    const closingBracket = endpoint.indexOf("]");
    if (closingBracket > 1) {
      parsedAddress = endpoint.slice(1, closingBracket);
      const explicitPort = endpoint.slice(closingBracket + 1).match(/^:(\d{1,5})$/)?.[1];
      if (explicitPort) parsedPort = explicitPort;
    }
  } else {
    const colonIndex = endpoint.lastIndexOf(":");
    const hasSingleColon = colonIndex > 0 && endpoint.indexOf(":") === colonIndex;
    const explicitPort = hasSingleColon
      ? endpoint.slice(colonIndex + 1).match(/^\d{1,5}$/)?.[0]
      : null;
    parsedAddress = explicitPort ? endpoint.slice(0, colonIndex) : endpoint;
    if (explicitPort) parsedPort = explicitPort;
  }
  reauthenticationPaneId.value = null;
  launcherTargetPaneId.value = paneId ?? activeLauncherPane()?.paneId ?? ensureLauncherTarget();
  requestedHost.value = null;
  requestedPluginAuthorizationToken.value = null;
  username.value = parsedUsername;
  address.value = parsedAddress;
  port.value = parsedPort;
  openLauncher();
}

function terminalFocusIsBlocked() {
  return launcherOpen.value
    || telnetLauncherOpen.value
    || vaultDialogOpen.value
    || closeConfirmationVisible.value;
}

function deactivateTerminalWorkspace() {
  void clearTerminalInputFocus();
  const tab = activeTab.value;
  if (tab) paneRefs.get(tab.activePaneId)?.deactivateFromTab();
}

function setPaneBellAttention(paneId: string, active: boolean) {
  if (!findWorkspacePane(paneId)) return;
  const next = new Set(bellAttentionPaneIds.value);
  if (active) next.add(paneId);
  else next.delete(paneId);
  bellAttentionPaneIds.value = next;
}

function clearTabBellAttention(tabId: string) {
  const tab = tabs.value.find((candidate) => candidate.tabId === tabId);
  if (!tab) return;
  const next = new Set(bellAttentionPaneIds.value);
  for (const pane of tab.panes) next.delete(pane.paneId);
  bellAttentionPaneIds.value = next;
}

function reconcileTerminalAfterForeground() {
  if (
    route.path !== "/terminal"
    || document.visibilityState === "hidden"
    || terminalFocusIsBlocked()
  ) return;
  void reconcilePluginProtocolLaunches();
  for (const pane of paneRefs.values()) void pane.reconcileAfterForeground?.();
}

function handleTerminalVisibilityChange() {
  if (document.visibilityState === "visible") reconcileTerminalAfterForeground();
}

function activateTab(tabId: string) {
  if (!tabs.value.some((tab) => tab.tabId === tabId)) return;
  clearTabBellAttention(tabId);
  void clearTerminalInputFocus();
  const previousTab = activeTab.value;
  if (previousTab && previousTab.tabId !== tabId) {
    paneRefs.get(previousTab.activePaneId)?.deactivateFromTab();
  }
  activeTabId.value = tabId;
  const activateCurrentPane = () => nextTick(() => {
    if (activeTabId.value !== tabId || terminalFocusIsBlocked()) return;
    const tab = tabs.value.find((candidate) => candidate.tabId === tabId);
    const pane = tab?.panes.find((candidate) => candidate.paneId === tab.activePaneId);
    if (pane && pane.kind !== "launcher") paneRefs.get(pane.paneId)?.activateFromTab();
  });
  if (route.path !== "/terminal") {
    void router.push("/terminal").then(activateCurrentPane);
    return;
  }
  void activateCurrentPane();
}

function activatePane(tabId: string, paneId: string) {
  const tab = tabs.value.find((candidate) => candidate.tabId === tabId);
  if (!tab || !findTerminalPane(tab.layout, paneId)) return;
  setPaneBellAttention(paneId, false);
  const previousPaneId = tab.activePaneId;
  if (previousPaneId !== paneId) {
    void clearTerminalInputFocus();
    paneRefs.get(previousPaneId)?.deactivateFromTab();
    tab.activePaneId = paneId;
  }
  if (tabId !== activeTabId.value || route.path !== "/terminal") {
    activateTab(tabId);
    return;
  }
  void nextTick(() => {
    if (activeTabId.value !== tabId || tab.activePaneId !== paneId || terminalFocusIsBlocked()) return;
    const pane = tab.panes.find((candidate) => candidate.paneId === paneId);
    if (pane && pane.kind !== "launcher") paneRefs.get(paneId)?.activateFromTab();
  });
}

function createLauncherTab(refreshHosts = true) {
  reauthenticationPaneId.value = null;
  const tabId = createUuidV7();
  const paneId = createUuidV7();
  tabs.value.push({
    tabId,
    layout: createTerminalPane(paneId),
    activePaneId: paneId,
    panes: [{ kind: "launcher", paneId, label: t("sshTerminal.newTabLabel") }],
  });
  activateTab(tabId);
  if (refreshHosts) void refreshSavedHosts();
  return paneId;
}

function createLocalTerminalTab() {
  reauthenticationPaneId.value = null;
  const tabId = createUuidV7();
  const paneId = createUuidV7();
  tabs.value.push({
    tabId,
    layout: createTerminalPane(paneId),
    activePaneId: paneId,
    panes: [{
      kind: "local",
      paneId,
      label: t("localSession.defaultShell"),
      state: "starting",
      summary: null,
      deferredStart: false,
    }],
  });
  activateTab(tabId);
  return paneId;
}

function createNewTerminalTab() {
  return ui.newTerminalBehavior === "localTerminal"
    ? createLocalTerminalTab()
    : createLauncherTab();
}

function createLocalFromHeader() {
  if (terminalFocusIsBlocked()) return false;
  createLocalTerminalTab();
  return true;
}

function quickConnectFromHeader() {
  if (terminalFocusIsBlocked()) return false;
  // This entry point explicitly creates a Launcher before opening the authentication form, ignoring the default-local-terminal preference.
  const paneId = createLauncherTab(false);
  openInlineQuickConnect("", paneId);
  return true;
}

function activeLauncherPane() {
  return activePane.value?.kind === "launcher" ? activePane.value : null;
}

function ensureLauncherTarget() {
  return activeLauncherPane()?.paneId ?? createLauncherTab();
}

function closeLauncher() {
  launcherOpen.value = false;
  reauthenticationPaneId.value = null;
  launcherTargetPaneId.value = null;
  requestedInitialDirectory.value = null;
}

function takeRequestedInitialDirectory(hostId: string, paneId: string): string | null {
  const request = requestedInitialDirectory.value;
  requestedInitialDirectory.value = null;
  return request?.hostId === hostId && request.paneId === paneId
    ? request.directory
    : null;
}

function setPaneRef(paneId: string, value: unknown) {
  if (value && typeof value === "object" && "activateFromTab" in value) {
    paneRefs.set(paneId, value as SshPaneExpose);
  } else {
    paneRefs.delete(paneId);
  }
}

function removeTab(tabId: string) {
  const index = tabs.value.findIndex((tab) => tab.tabId === tabId);
  if (index < 0) return;
  const tab = tabs.value[index]!;
  clearTabBellAttention(tabId);
  paneRefs.get(tab.activePaneId)?.deactivateFromTab();
  for (const pane of tab.panes) paneRefs.delete(pane.paneId);
  tabs.value.splice(index, 1);
  if (activeTabId.value === tabId) {
    const nextTabId = tabs.value[Math.min(index, tabs.value.length - 1)]?.tabId ?? "";
    if (route.path === "/terminal") activateTab(nextTabId);
    else activeTabId.value = nextTabId;
  }
}

function paneIsClosed(pane: TerminalWorkspacePane) {
  return pane.kind === "launcher"
    || (pane.kind === "session" && ["closed", "failed"].includes(pane.state))
    || (pane.kind === "telnet" && ["closed", "failed"].includes(pane.state))
    || (pane.kind === "plugin" && ["closed", "failed"].includes(pane.state) && !pane.summary?.cleanupBlocked)
    || (pane.kind === "local" && (
      ["exited", "closed"].includes(pane.state)
      || (pane.state === "failed" && pane.summary?.failureReason?.code !== "processCleanupFailed")
    ));
}

function closePaneView(tabId: string, paneId: string) {
  const tab = tabs.value.find((candidate) => candidate.tabId === tabId);
  if (!tab) return;
  setPaneBellAttention(paneId, false);
  paneRefs.get(paneId)?.deactivateFromTab();
  if (tab.activePaneId === paneId) void clearTerminalInputFocus();
  paneRefs.delete(paneId);
  if (tab.panes.length === 1) {
    removeTab(tabId);
    return;
  }
  const closed = closeTerminalPane(tab.layout, paneId);
  tab.layout = closed.node;
  tab.panes = tab.panes.filter((pane) => pane.paneId !== paneId);
  if (tab.activePaneId === paneId) tab.activePaneId = closed.nextActivePaneId;
  if (tabId === activeTabId.value) activatePane(tabId, tab.activePaneId);
}

function requestClosePane(tabId: string, paneId: string) {
  const tab = tabs.value.find((candidate) => candidate.tabId === tabId);
  const pane = tab?.panes.find((candidate) => candidate.paneId === paneId);
  if (!tab || !pane) return;
  if (paneIsClosed(pane)) {
    closePaneView(tabId, paneId);
    return;
  }
  activatePane(tabId, paneId);
  closeTabErrorVisible.value = false;
  skipFutureSinglePaneTabClosePrompt.value = false;
  closeCandidate.value = { mode: "pane", tabIds: [tabId], paneIds: [paneId] };
  closeConfirmationVisible.value = true;
}

function requestCloseTab(tabId: string) {
  if (closeCandidate.value || closingTab.value) return;
  const tab = tabs.value.find((candidate) => candidate.tabId === tabId);
  if (!tab) return;
  const openPaneIds = tab.panes.filter((pane) => !paneIsClosed(pane)).map((pane) => pane.paneId);
  if (openPaneIds.length === 0) {
    removeTab(tabId);
    return;
  }
  activateTab(tabId);
  closeTabErrorVisible.value = false;
  skipFutureSinglePaneTabClosePrompt.value = false;
  const candidate: CloseCandidate = { mode: "tab", tabIds: [tabId], paneIds: openPaneIds };
  closeCandidate.value = candidate;
  if (tab.panes.length === 1 && ui.singlePaneTabCloseBehavior === "closeDirectly") {
    closeConfirmationVisible.value = false;
    void confirmCloseTab();
    return;
  }
  closeConfirmationVisible.value = true;
}

function requestCloseTabs(tabIds: readonly string[]) {
  if (closeCandidate.value || closingTab.value) return;
  const targetIds = [...new Set(tabIds)].filter((tabId) => (
    tabs.value.some((tab) => tab.tabId === tabId)
  ));
  if (!targetIds.length) return;
  const paneIds = tabs.value
    .filter((tab) => targetIds.includes(tab.tabId))
    .flatMap((tab) => tab.panes.filter((pane) => !paneIsClosed(pane)).map((pane) => pane.paneId));
  if (!paneIds.length) {
    for (const tabId of targetIds) removeTab(tabId);
    return;
  }
  activateTab(targetIds[0]!);
  closeTabErrorVisible.value = false;
  skipFutureSinglePaneTabClosePrompt.value = false;
  closeCandidate.value = { mode: "tabs", tabIds: targetIds, paneIds };
  closeConfirmationVisible.value = true;
}

function cancelCloseTab() {
  if (closingTab.value) return;
  clearCloseTerminalsTimeout();
  closeCandidate.value = null;
  closeConfirmationVisible.value = false;
  skipFutureSinglePaneTabClosePrompt.value = false;
  closeTabErrorVisible.value = false;
  activateTab(activeTabId.value);
}

function clearCloseTerminalsTimeout() {
  if (closeTerminalsTimeout !== null) window.clearTimeout(closeTerminalsTimeout);
  closeTerminalsTimeout = null;
}

function startCloseTerminalsTimeout(candidate: CloseCandidate, onTimeout?: () => void) {
  clearCloseTerminalsTimeout();
  closeTerminalsTimeout = window.setTimeout(() => {
    if (closeCandidate.value !== candidate) return;
    if (onTimeout) {
      closeTerminalsTimeout = null;
      onTimeout();
      return;
    }
    closingTab.value = false;
    closeTabErrorVisible.value = true;
    closeConfirmationVisible.value = true;
    closeTerminalsTimeout = null;
  }, CLOSE_TERMINALS_TIMEOUT_MS);
}

function captureOptimisticClose(candidate: CloseCandidate): OptimisticCloseSnapshot {
  return {
    activeTabId: activeTabId.value,
    tabs: candidate.tabIds.flatMap((tabId) => {
      const index = tabs.value.findIndex((tab) => tab.tabId === tabId);
      return index < 0 ? [] : [{ index, tab: tabs.value[index]! }];
    }),
  };
}

function restoreOptimisticClose(
  candidate: CloseCandidate,
  snapshot: OptimisticCloseSnapshot,
  attempt: number,
) {
  if (closeCandidate.value !== candidate || optimisticTabCloseAttempt !== attempt) return;
  clearCloseTerminalsTimeout();
  optimisticTabCloseInFlight = false;
  const existingTabIds = new Set(tabs.value.map((tab) => tab.tabId));
  for (const entry of [...snapshot.tabs].sort((left, right) => left.index - right.index)) {
    if (existingTabIds.has(entry.tab.tabId)) continue;
    tabs.value.splice(Math.min(entry.index, tabs.value.length), 0, entry.tab);
    existingTabIds.add(entry.tab.tabId);
  }
  closingTab.value = false;
  closeTabErrorVisible.value = true;
  closeConfirmationVisible.value = true;
  skipFutureSinglePaneTabClosePrompt.value = false;
  const restoredActiveTabId = snapshot.tabs.some(
    (entry) => entry.tab.tabId === snapshot.activeTabId,
  ) ? snapshot.activeTabId : candidate.tabIds[0]!;
  void nextTick(() => activateTab(restoredActiveTabId));
}

function closeOperationForPane(paneId: string) {
  const pane = paneRefs.get(paneId);
  if (!pane) throw new Error("Terminal Pane is unavailable");
  if (pane.disconnectForClose) return () => pane.disconnectForClose!();
  if (pane.terminateForClose) return () => pane.terminateForClose!();
  throw new Error("Terminal Pane cannot be closed safely");
}

async function confirmCloseTab() {
  const candidate = closeCandidate.value;
  if (!candidate || closingTab.value || closePreferenceSaving.value) return;
  if (closeCandidateCanDisablePrompt.value && skipFutureSinglePaneTabClosePrompt.value) {
    closePreferenceSaving.value = true;
    try {
      if (!await ui.setSinglePaneTabCloseBehavior("closeDirectly")) {
        tips.show({ scope: "terminal-close-preference", tone: "error", title: t("sshTerminal.doNotAskAgainSaveFailed") });
      }
    } catch (error) {
      tips.show({ scope: "terminal-close-preference", tone: "error", title: t("sshTerminal.doNotAskAgainSaveFailed"),
        message: t(`applicationPreferenceErrors.${applicationPreferenceFailure(error)}`) });
    } finally {
      closePreferenceSaving.value = false;
    }
    if (closeCandidate.value !== candidate) return;
  }
  let closeOperations: Array<() => Promise<void>>;
  try {
    closeOperations = candidate.paneIds.map(closeOperationForPane);
  } catch {
    closeTabErrorVisible.value = true;
    closeConfirmationVisible.value = true;
    return;
  }

  closingTab.value = true;
  closeTabErrorVisible.value = false;

  if (candidate.mode !== "pane") {
    const snapshot = captureOptimisticClose(candidate);
    const attempt = ++optimisticTabCloseAttempt;
    optimisticTabCloseInFlight = true;
    closeConfirmationVisible.value = false;
    for (const tabId of candidate.tabIds) removeTab(tabId);
    startCloseTerminalsTimeout(
      candidate,
      () => restoreOptimisticClose(candidate, snapshot, attempt),
    );
    try {
      await Promise.all(closeOperations.map((close) => close()));
      if (
        closeCandidate.value !== candidate
        || !optimisticTabCloseInFlight
        || optimisticTabCloseAttempt !== attempt
      ) return;
      clearCloseTerminalsTimeout();
      optimisticTabCloseInFlight = false;
      closeCandidate.value = null;
      closingTab.value = false;
      skipFutureSinglePaneTabClosePrompt.value = false;
      queueWorkspaceSave(workspaceProjection.value);
    } catch {
      restoreOptimisticClose(candidate, snapshot, attempt);
    }
    return;
  }

  startCloseTerminalsTimeout(candidate);
  try {
    await Promise.all(closeOperations.map((close) => close()));
    await nextTick();
    resolvePendingClose();
    if (closeCandidate.value) {
      const accepted = candidate.paneIds.every((paneId) => {
        const pane = tabs.value
          .flatMap((tab) => tab.panes)
          .find((value) => value.paneId === paneId);
        return pane === undefined
          || paneIsClosed(pane)
          || (pane.kind === "session" && pane.state === "disconnecting")
          || ((pane.kind === "telnet" || pane.kind === "plugin") && pane.state === "disconnecting")
          || (pane.kind === "local" && pane.state === "stopping");
      });
      if (!accepted) {
        clearCloseTerminalsTimeout();
        closingTab.value = false;
        closeTabErrorVisible.value = true;
        closeConfirmationVisible.value = true;
      }
    }
  } catch {
    clearCloseTerminalsTimeout();
    closingTab.value = false;
    closeTabErrorVisible.value = true;
    closeConfirmationVisible.value = true;
  }
}

function resolvePendingClose() {
  const candidate = closeCandidate.value;
  if (!candidate) return;
  if (!candidate.tabIds.some((tabId) => tabs.value.some((tab) => tab.tabId === tabId))) {
    clearCloseTerminalsTimeout();
    closeCandidate.value = null;
    closeConfirmationVisible.value = false;
    skipFutureSinglePaneTabClosePrompt.value = false;
    closingTab.value = false;
    return;
  }
  if (candidate.paneIds.some((paneId) => {
    const pane = tabs.value
      .flatMap((tab) => tab.panes)
      .find((value) => value.paneId === paneId);
    return pane !== undefined && !paneIsClosed(pane);
  })) return;
  if (candidate.mode === "pane") closePaneView(candidate.tabIds[0]!, candidate.paneIds[0]!);
  else for (const tabId of candidate.tabIds) removeTab(tabId);
  clearCloseTerminalsTimeout();
  closeCandidate.value = null;
  closeConfirmationVisible.value = false;
  skipFutureSinglePaneTabClosePrompt.value = false;
  closingTab.value = false;
}

function openHosts() {
  void router.push({ path: "/hosts", query: { create: "1" } });
}

function showAllHosts() {
  void router.push({ path: "/hosts" });
}

function openSshConfigImport() {
  void router.push({ path: "/hosts", query: { importSshConfig: "1" } });
}

function terminalLauncherVisualFixture(): HostCatalogEntry[] {
  const now = Date.now();
  return [
    {
      host: {
        hostId: "019d0000-0000-7000-8000-000000000701",
        label: "生产环境",
        address: "prod.example.com",
        normalizedAddress: "prod.example.com",
        port: 22,
        username: "root",
        identityId: null,
        favorite: true,
        hasReadyCredential: true,
        stateVersion: "1",
      },
      group: null,
      tags: [],
      recentConnection: {
        hostId: "019d0000-0000-7000-8000-000000000701",
        connectedAtUnixMs: now - 2 * 60_000,
        recencySequence: "3",
        successfulConnectionCount: "18",
      },
    },
    {
      host: {
        hostId: "019d0000-0000-7000-8000-000000000702",
        label: "香港节点",
        address: "hk-node-01.example.com",
        normalizedAddress: "hk-node-01.example.com",
        port: 22,
        username: "ubuntu",
        identityId: null,
        favorite: false,
        hasReadyCredential: true,
        stateVersion: "1",
      },
      group: null,
      tags: [],
      recentConnection: {
        hostId: "019d0000-0000-7000-8000-000000000702",
        connectedAtUnixMs: now - 60 * 60_000,
        recencySequence: "2",
        successfulConnectionCount: "9",
      },
    },
    {
      host: {
        hostId: "019d0000-0000-7000-8000-000000000703",
        label: "开发服务器",
        address: "dev-server.example.com",
        normalizedAddress: "dev-server.example.com",
        port: 2222,
        username: "dev",
        identityId: null,
        favorite: false,
        hasReadyCredential: false,
        stateVersion: "1",
      },
      group: null,
      tags: [],
      recentConnection: {
        hostId: "019d0000-0000-7000-8000-000000000703",
        connectedAtUnixMs: now - 24 * 60 * 60_000,
        recencySequence: "1",
        successfulConnectionCount: "4",
      },
    },
  ];
}

function showVaultStatusUnavailable() {
  vaultPromptMode.value = null;
  vaultDialogOpen.value = false;
  connectionErrorKey.value = "sshTerminal.vaultUnavailable";
  connectionErrorBodyKey.value = null;
  connectionErrorVisible.value = true;
}

function openVaultPrompt(
  purpose: "saveCredential" | "useSavedCredential",
  state: VaultState,
) {
  if (state !== "missing" && state !== "locked" && state !== "requiresReload") {
    showVaultStatusUnavailable();
    return false;
  }
  vaultPurpose.value = purpose;
  vaultPromptMode.value = state === "missing" ? "create" : "unlock";
  vaultErrorVisible.value = false;
  launcherOpen.value = false;
  vaultDialogOpen.value = true;
  void saveVaultAndConnect();
  return true;
}

async function refreshVaultState(): Promise<VaultState | null> {
  vaultStatusLoading.value = true;
  vaultStatusUnavailable.value = false;
  vaultState.value = null;
  if (!canUseDesktopCore()) {
    vaultStatusUnavailable.value = true;
    vaultStatusLoading.value = false;
    return null;
  }
  try {
    const state = (await fetchVaultStatus()).state;
    vaultState.value = state;
    return state;
  } catch {
    vaultStatusUnavailable.value = true;
    return null;
  } finally {
    vaultStatusLoading.value = false;
  }
}

let savedHostsSequence = 0;
let unlistenSavedConnections: (() => void) | null = null;
let terminalViewDisposed = false;
async function refreshSavedHosts() {
  const sequence = ++savedHostsSequence;
  if (import.meta.env.DEV && route.query.visualFixture === "terminalLauncher") {
    const entries = terminalLauncherVisualFixture();
    recentHostEntries.value = entries;
    savedHosts.value = entries.map((entry) => entry.host);
    return;
  }
  if (!canUseDesktopCore()) return;
  try {
    const entries = await listHostCatalog("recentlyConnected");
    if (terminalViewDisposed || sequence !== savedHostsSequence) return;
    recentHostEntries.value = entries
      .filter((entry) => entry.recentConnection !== null)
      .slice(0, 8);
    savedHosts.value = entries.slice(0, 8).map((entry) => entry.host);
  } catch {
    if (!terminalViewDisposed && sequence === savedHostsSequence) {
      recentHostEntries.value = [];
      savedHosts.value = [];
    }
  }
}

function currentLabel() {
  return requestedHost.value?.label ?? `${username.value.trim()}@${address.value.trim()}`;
}

function parsedConnectionPort() {
  return Number(port.value);
}

function connectionFormIsValid() {
  const parsedPort = parsedConnectionPort();
  return Boolean(
    address.value.trim()
    && username.value.trim()
    && Number.isInteger(parsedPort)
    && parsedPort >= 1
    && parsedPort <= 65535,
  );
}

async function ensureHostSavedWithoutCredential() {
  if (requestedHost.value) return requestedHost.value;
  const host = await createHost({
    label: currentLabel(),
    address: address.value.trim(),
    port: parsedConnectionPort(),
    username: username.value.trim(),
  });
  requestedHost.value = host;
  return host;
}

async function connectWithCredentialRef(credentialRefId: string) {
  const reconnectPaneId = reauthenticationPaneId.value;
  if (reconnectPaneId) {
    const pane = paneRefs.get(reconnectPaneId);
    if (!pane?.reconnectWithCredential) throw new Error("SSH terminal Pane is unavailable");
    await pane.reconnectWithCredential(credentialRefId);
    const target = findWorkspacePane(reconnectPaneId);
    if (target?.pane.kind === "session") target.pane.credentialRefId = credentialRefId;
    reauthenticationPaneId.value = null;
    launcherOpen.value = false;
    vaultDialogOpen.value = false;
    return;
  }
  const labelValue = currentLabel();
  const host = requestedHost.value;
  // Temporary credentials do not change a saved Host identity; only editing the endpoint turns this into Quick Connect.
  const matchesRequestedHost = host
    && host.address === address.value.trim()
    && host.port === parsedConnectionPort()
    && host.username === username.value.trim();
  if (requestedInitialDirectory.value && !matchesRequestedHost) {
    tips.show({ scope: "ssh-sftp-directory-launch", tone: "error", title: t("sshTerminal.sftpDirectoryConnectFailed") });
    return;
  }
  const paneId = launcherTargetPaneId.value ?? ensureLauncherTarget();
  const connectedPane: SshSessionPane = {
    kind: "session",
    paneId,
    label: labelValue,
    state: "resolving",
    target: matchesRequestedHost ? {
      kind: "host",
      hostId: host.hostId,
      expectedHostStateVersion: host.stateVersion,
    } : {
      kind: "quickConnect",
      endpoint: {
        address: address.value.trim(),
        port: parsedConnectionPort(),
        username: username.value.trim(),
      },
    },
    credentialRefId,
    summary: null,
    deferredStart: false,
    deferredRecovery: "reconnect",
    initialDirectory: matchesRequestedHost ? takeRequestedInitialDirectory(host.hostId, paneId) : null,
  };
  if (!matchesRequestedHost) requestedInitialDirectory.value = null;
  replaceWorkspacePane(paneId, connectedPane);
  launcherOpen.value = false;
  vaultDialogOpen.value = false;
  launcherTargetPaneId.value = null;
  await nextTick();
}

function createLocalTerminal(targetPaneId: string | null = null) {
  requestedInitialDirectory.value = null;
  const paneId = targetPaneId ?? activeLauncherPane()?.paneId ?? ensureLauncherTarget();
  const localPane: LocalSessionPane = {
    kind: "local",
    paneId,
    label: t("localSession.defaultShell"),
    state: "starting",
    summary: null,
    deferredStart: false,
  };
  replaceWorkspacePane(paneId, localPane);
}

function openTelnetLauncher(targetPaneId: string | null = null) {
  requestedInitialDirectory.value = null;
  launcherTargetPaneId.value = targetPaneId
    ?? activeLauncherPane()?.paneId
    ?? ensureLauncherTarget();
  telnetAddress.value = "";
  telnetPort.value = "23";
  telnetAcceptsCleartext.value = false;
  telnetAcceptsMissingIdentity.value = false;
  telnetAcceptsTampering.value = false;
  telnetValidationVisible.value = false;
  telnetLauncherOpen.value = true;
}

function closeTelnetLauncher() {
  telnetLauncherOpen.value = false;
  telnetValidationVisible.value = false;
  launcherTargetPaneId.value = null;
}

function createTelnetTerminal() {
  const parsedPort = Number(telnetPort.value);
  const valid = telnetAddress.value.trim().length > 0
    && Number.isInteger(parsedPort)
    && parsedPort >= 1
    && parsedPort <= 65_535
    && telnetAcceptsCleartext.value
    && telnetAcceptsMissingIdentity.value
    && telnetAcceptsTampering.value;
  if (!valid) {
    telnetValidationVisible.value = true;
    return;
  }
  const paneId = launcherTargetPaneId.value
    ?? activeLauncherPane()?.paneId
    ?? ensureLauncherTarget();
  const endpoint = { address: telnetAddress.value.trim(), port: parsedPort };
  const pane: TelnetSessionPane = {
    kind: "telnet",
    paneId,
    label: `${endpoint.address}:${endpoint.port}`,
    endpoint,
    state: "connecting",
    summary: null,
    deferredStart: false,
  };
  replaceWorkspacePane(paneId, pane);
  telnetLauncherOpen.value = false;
  launcherTargetPaneId.value = null;
}

function findWorkspacePane(paneId: string) {
  for (const tab of tabs.value) {
    const pane = tab.panes.find((candidate) => candidate.paneId === paneId);
    if (pane) return { tab, pane };
  }
  return null;
}

function replaceWorkspacePane(paneId: string, pane: TerminalWorkspacePane) {
  const target = findWorkspacePane(paneId);
  if (!target) return;
  const index = target.tab.panes.findIndex((candidate) => candidate.paneId === paneId);
  target.tab.panes.splice(index, 1, pane);
  target.tab.layout = setTerminalForPane(target.tab.layout, paneId, paneId);
  target.tab.activePaneId = paneId;
  activatePane(target.tab.tabId, paneId);
}

// Recovery may await Host/Vault data before a dialog exists. Serialize explicit
// requests during that gap so a burst of typing cannot create competing prompts.
let sessionRecoveryPending = false;
async function requestSessionRecovery(paneId: string, target: SshSessionTarget, kind: "credential" | "vault") {
  if (sessionRecoveryPending || terminalFocusIsBlocked()) return;
  sessionRecoveryPending = true;
  try {
    if (kind === "credential") await requestSessionCredential(paneId, target);
    else await requestSessionVaultUnlock(paneId, target);
  } finally {
    sessionRecoveryPending = false;
  }
}

async function requestSessionCredential(
  paneId: string,
  target: SshSessionTarget,
) {
  const workspace = findWorkspacePane(paneId);
  if (!workspace || workspace.pane.kind !== "session") return;
  activatePane(workspace.tab.tabId, paneId);
  reauthenticationPaneId.value = paneId;
  requestedHost.value = null;
  if (target.kind === "quickConnect") {
    address.value = target.endpoint.address;
    port.value = String(target.endpoint.port);
    username.value = target.endpoint.username ?? "";
  } else {
    const endpoint = workspace.pane.kind === "session" ? workspace.pane.summary?.endpoint : null;
    if (endpoint) {
      address.value = endpoint.address;
      port.value = String(endpoint.port);
      username.value = endpoint.username ?? "";
    }
    if (canUseDesktopCore()) {
      try {
        const host = (await listHosts()).find((candidate) => candidate.hostId === target.hostId);
        if (host) {
          requestedHost.value = host;
          address.value = host.address;
          port.value = String(host.port);
          username.value = host.username ?? "";
        }
      } catch {
        connectionErrorVisible.value = true;
      }
    }
  }
  openLauncher();
}

async function requestSessionVaultUnlock(
  paneId: string,
  target: SshSessionTarget,
) {
  const workspace = findWorkspacePane(paneId);
  if (!workspace || workspace.pane.kind !== "session") return;
  activatePane(workspace.tab.tabId, paneId);
  reauthenticationPaneId.value = paneId;
  pendingVaultReconnectPaneIds.clear();
  pendingVaultReconnectPaneIds.add(paneId);
  requestedHost.value = null;
  if (target.kind === "host" && canUseDesktopCore()) {
    try {
      requestedHost.value = (await listHosts()).find(
        (candidate) => candidate.hostId === target.hostId,
      ) ?? null;
    } catch {
      connectionErrorVisible.value = true;
    }
  }
  const state = await refreshVaultState();
  if (state === "unlocked") {
    workspace.pane.deferredRecovery = "reconnect";
    const pane = paneRefs.get(paneId);
    if (!pane?.reconnectSavedCredential) {
      showVaultStatusUnavailable();
      return;
    }
    try {
      await pane.reconnectSavedCredential();
    } catch {
      showVaultStatusUnavailable();
    }
    return;
  }
  if (state === "missing") {
    workspace.pane.deferredRecovery = "credential";
    pendingVaultReconnectPaneIds.delete(paneId);
    await requestSessionCredential(paneId, target);
    return;
  }
  if (state === "locked" || state === "requiresReload") {
    workspace.pane.deferredRecovery = "vaultUnlock";
    openVaultPrompt("useSavedCredential", state);
    return;
  }
  showVaultStatusUnavailable();
}

async function requestSessionAuthenticationRecovery(
  paneId: string,
  target: SshSessionTarget,
) {
  const workspace = findWorkspacePane(paneId);
  if (!workspace || workspace.pane.kind !== "session") return;
  activatePane(workspace.tab.tabId, paneId);
  workspace.pane.deferredStart = true;
  const state = await refreshVaultState();
  if (state === "locked" || state === "requiresReload") {
    workspace.pane.deferredRecovery = "vaultUnlock";
    if (vaultDialogOpen.value && vaultPurpose.value === "useSavedCredential") {
      pendingVaultReconnectPaneIds.add(paneId);
      reauthenticationPaneId.value ??= paneId;
      return;
    }
    await requestSessionVaultUnlock(paneId, target);
    return;
  }
  workspace.pane.deferredRecovery = "credential";
  requestSessionCredential(paneId, target);
}

async function connectSavedHost(
  host: HostSummary,
  pluginAuthorizationToken: string | null = requestedPluginAuthorizationToken.value,
) {
  const paneId = launcherTargetPaneId.value ?? activeLauncherPane()?.paneId ?? ensureLauncherTarget();
  const connectedPane: SshSessionPane = {
    kind: "session",
    paneId,
    label: host.label,
    state: "resolving",
    target: {
      kind: "host",
      hostId: host.hostId,
      expectedHostStateVersion: host.stateVersion,
    },
    credentialRefId: null,
    pluginAuthorizationToken,
    summary: null,
    deferredStart: false,
    deferredRecovery: "reconnect",
    initialDirectory: takeRequestedInitialDirectory(host.hostId, paneId),
  };
  replaceWorkspacePane(paneId, connectedPane);
  requestedPluginAuthorizationToken.value = null;
  launcherTargetPaneId.value = null;
  await nextTick();
}

async function connectWithoutSavingCredential() {
  const credentialRefId = await requestSecureCredential({ kind: authentication.value === "privateKey" ? "privateKey" : "password", label: currentLabel(), identityId: null });
  if (credentialRefId) await connectWithCredentialRef(credentialRefId);
}

async function connectAndSaveCredential() {
  const host = await ensureHostSavedWithoutCredential();
  const identityId = host.identityId ?? (await createIdentity(
    currentLabel(),
    username.value.trim(),
  )).identityId;
  const credentialRefId = await requestSecureCredential({ kind: authentication.value === "privateKey" ? "privateKey" : "password", label: currentLabel(), identityId });
  if (!credentialRefId) return;
  credentialSavedDuringAttempt.value = true;
  let effectiveHost = host;
  if (host.identityId !== identityId) {
    effectiveHost = await updateHost({
      hostId: host.hostId,
      expectedStateVersion: host.stateVersion,
      label: host.label,
      address: host.address,
      port: host.port,
      username: host.username,
      identityId,
      favorite: host.favorite,
    });
    requestedHost.value = effectiveHost;
  }
  const connectionConfig = await getHostConnectionConfig(effectiveHost.hostId);
  if (connectionConfig.authenticationPlan.mode === "hostOverride") {
    const reconnectPane = reauthenticationPaneId.value
      ? findWorkspacePane(reauthenticationPaneId.value)?.pane
      : null;
    const rejectedCredentialRefId = reconnectPane?.kind === "session"
      ? reconnectPane.credentialRefId
      : null;
    const credentialRefIds = [
      credentialRefId,
      ...connectionConfig.authenticationPlan.credentialRefIds.filter(
        (existingCredentialRefId) => existingCredentialRefId !== credentialRefId
          && existingCredentialRefId !== rejectedCredentialRefId,
      ),
    ].slice(0, 16);
    await replaceAuthenticationPlan({
      hostId: effectiveHost.hostId,
      expectedRevision: connectionConfig.authenticationPlan.revision,
      mode: "hostOverride",
      credentialRefIds,
    });
  }
  await connectWithCredentialRef(credentialRefId);
  credentialSavedDuringAttempt.value = false;
}

async function attemptConnect() {
  if (preparing.value) return;
  validationVisible.value = !connectionFormIsValid();
  if (validationVisible.value) return;
  const requestedDirectory = requestedInitialDirectory.value;
  const host = requestedHost.value;
  if (requestedDirectory && (!host || host.hostId !== requestedDirectory.hostId
    || host.address !== address.value.trim()
    || host.port !== parsedConnectionPort()
    || host.username !== username.value.trim())) {
    tips.show({ scope: "ssh-sftp-directory-launch", tone: "error", title: t("sshTerminal.sftpDirectoryConnectFailed") });
    return;
  }
  preparing.value = true;
  credentialSavedDuringAttempt.value = false;
  connectionErrorVisible.value = false;
  try {
    if (!saveCredential.value) {
      await connectWithoutSavingCredential();
      return;
    }
    await ensureHostSavedWithoutCredential();
    const state = await refreshVaultState();
    if (state === "unlocked") {
      await connectAndSaveCredential();
      return;
    }
    if (state === "missing" || state === "locked" || state === "requiresReload") {
      openVaultPrompt("saveCredential", state);
      return;
    }
    showVaultStatusUnavailable();
  } catch (error) {
    showConnectionError(error);
  } finally {
    preparing.value = false;
  }
}

async function saveVaultAndConnect() {
  const purpose = vaultPurpose.value;
  try {
    const approved = await requestSecureVault("ensureUnlocked");
    if (!vaultDialogOpen.value) return;
    vaultDialogOpen.value = false;
    vaultPromptMode.value = null;
    if (!approved) { launcherOpen.value = true; return; }
    vaultState.value = "unlocked";
    preparing.value = true;
    if (purpose === "useSavedCredential" && pendingVaultReconnectPaneIds.size > 0) {
      for (const paneId of [...pendingVaultReconnectPaneIds]) {
        const workspace = findWorkspacePane(paneId);
        if (workspace?.pane.kind === "session") {
          workspace.pane.deferredRecovery = "reconnect";
        }
        await nextTick();
        const pane = paneRefs.get(paneId);
        if (!pane?.reconnectSavedCredential) throw new Error("SSH terminal Pane is unavailable");
        await pane.reconnectSavedCredential();
        pendingVaultReconnectPaneIds.delete(paneId);
      }
      reauthenticationPaneId.value = null;
    } else if (purpose === "useSavedCredential" && requestedHost.value) {
      await connectSavedHost(requestedHost.value);
    } else {
      await connectAndSaveCredential();
    }
  } catch (error) {
    vaultDialogOpen.value = false;
    showConnectionError(error);
    if (!reauthenticationPaneId.value) launcherOpen.value = true;
  } finally { preparing.value = false; }
}



function updateTabState(paneId: string, state: SshSessionState, summary: SshSessionSummary | null) {
  const target = findWorkspacePane(paneId);
  if (!target || target.pane.kind !== "session") return;
  target.pane.state = state;
  target.pane.summary = summary;
  if (["closed", "failed"].includes(state)) void nextTick(resolvePendingClose);
}

function updateLocalTabState(
  paneId: string,
  state: LocalSessionState,
  summary: LocalSessionSummary | null,
) {
  const target = findWorkspacePane(paneId);
  if (!target || target.pane.kind !== "local") return;
  target.pane.state = state;
  target.pane.summary = summary;
  if (summary?.shellName) target.pane.label = summary.shellName;
  if (paneIsClosed(target.pane)) void nextTick(resolvePendingClose);
}

function updateTelnetTabState(
  paneId: string,
  state: TelnetSessionState,
  summary: TelnetSessionSummary | null,
) {
  const target = findWorkspacePane(paneId);
  if (!target || target.pane.kind !== "telnet") return;
  target.pane.state = state;
  target.pane.summary = summary;
  if (summary) {
    target.pane.endpoint = summary.endpoint;
    target.pane.label = `${summary.endpoint.address}:${summary.endpoint.port}`;
  }
  if (paneIsClosed(target.pane)) void nextTick(resolvePendingClose);
}

function pluginProfile(summary: PluginTerminalSessionSummary): PluginTerminalProfile {
  return { pluginId: summary.pluginId, providerId: summary.providerId,
    schemaHash: summary.schemaHash, configuration: summary.configuration };
}

function updatePluginTabState(paneId: string, state: PluginTerminalSessionState, summary: PluginTerminalSessionSummary | null) {
  const target = findWorkspacePane(paneId);
  if (!target || target.pane.kind !== "plugin") return;
  target.pane.state = state;
  target.pane.summary = summary;
  if (summary) { target.pane.profile = pluginProfile(summary); target.pane.label = summary.label; target.pane.launch = null; }
  if (paneIsClosed(target.pane)) void nextTick(resolvePendingClose);
}

function projectPluginSession(summary: PluginTerminalSessionSummary) {
  const pane: PluginSessionPane = { kind: "plugin", paneId: summary.paneId, label: summary.label,
    profile: pluginProfile(summary), launch: null, state: summary.state, summary, deferredStart: false };
  const existing = findWorkspacePane(summary.paneId);
  if (existing) {
    existing.tab.panes.splice(existing.tab.panes.findIndex((item) => item.paneId === summary.paneId), 1, pane);
  } else {
    tabs.value.push({ tabId: summary.tabId, activePaneId: summary.paneId,
      layout: createTerminalPane(summary.paneId, summary.paneId), panes: [pane] });
  }
}

let protocolLaunchReconciliation: Promise<void> | null = null;
function reconcilePluginProtocolLaunches() {
  if (!canUseDesktopCore()) return Promise.resolve();
  if (protocolLaunchReconciliation) return protocolLaunchReconciliation;
  protocolLaunchReconciliation = (async () => {
    await ensureTerminalWorkspaceInitialized();
    const launches = await listPluginProtocolLaunches();
    pluginLaunchErrorVisible.value = false;
    for (const launch of launches) {
      if (launch.claimed || launch.expiresAtUnixMs <= Date.now() || findWorkspacePane(launch.paneId)) continue;
      const pane: PluginSessionPane = { kind: "plugin", paneId: launch.paneId,
        label: launch.label, profile: null, launch, state: "connecting", summary: null, deferredStart: false };
      tabs.value.push({ tabId: launch.tabId, activePaneId: launch.paneId,
        layout: createTerminalPane(launch.paneId, launch.paneId), panes: [pane] });
      // Vue mounts a visible Connecting Pane before that Pane claims the Core record.
      activateTab(launch.tabId);
    }
  })().catch(() => { pluginLaunchErrorVisible.value = true; })
    .finally(() => { protocolLaunchReconciliation = null; });
  return protocolLaunchReconciliation;
}

function activeLocalSessions(snapshot: LocalSessionSnapshot) {
  return snapshot.sessions.filter((candidate) =>
    !["exited", "closed"].includes(candidate.state)
    && (candidate.state !== "failed"
      || candidate.failureReason?.code === "processCleanupFailed"));
}

function activeSshSessions(snapshot: SshSessionSnapshot) {
  return snapshot.sessions.filter(
    (summary) => !["closed", "failed"].includes(summary.state),
  );
}

function activeTelnetSessions(snapshot: TelnetSessionSnapshot) {
  return snapshot.sessions.filter(
    (summary) => !["closed", "failed"].includes(summary.state),
  );
}

function recoverRendererSessions(
  sshSnapshot: SshSessionSnapshot,
  localSnapshot: LocalSessionSnapshot,
  telnetSnapshot: TelnetSessionSnapshot,
  sshPaneBySessionId: ReadonlyMap<string, string> = new Map(),
  localPaneBySessionId: ReadonlyMap<string, string> = new Map(),
  telnetPaneBySessionId: ReadonlyMap<string, string> = new Map(),
) {
  const activeSessions = activeSshSessions(sshSnapshot);
  for (const summary of activeSessions) {
    const endpoint = summary.endpoint;
    const label = endpoint
      ? `${endpoint.username ? `${endpoint.username}@` : ""}${endpoint.address}`
      : summary.sessionId;
    const paneId = sshPaneBySessionId.get(summary.sessionId) ?? createUuidV7();
    const recoveredPane: SshSessionPane = {
      kind: "session",
      paneId,
      label,
      state: summary.state,
      target: summary.target,
      credentialRefId: summary.credentialRefId,
      summary,
      deferredStart: false,
      deferredRecovery: "reconnect",
    };
    const existing = findWorkspacePane(paneId);
    if (existing) {
      const paneIndex = existing.tab.panes.findIndex((pane) => pane.paneId === paneId);
      existing.tab.panes.splice(paneIndex, 1, recoveredPane);
      existing.tab.layout = setTerminalForPane(existing.tab.layout, paneId, paneId);
      continue;
    }
    tabs.value.push({
      tabId: createUuidV7(),
      layout: createTerminalPane(paneId, paneId),
      activePaneId: paneId,
      panes: [recoveredPane],
    });
  }
  for (const summary of activeLocalSessions(localSnapshot)) {
    const paneId = localPaneBySessionId.get(summary.sessionId) ?? createUuidV7();
    const recoveredPane: LocalSessionPane = {
      kind: "local",
      paneId,
      label: summary.shellName || t("localSession.defaultShell"),
      state: summary.state,
      summary,
      deferredStart: false,
    };
    const existing = findWorkspacePane(paneId);
    if (existing) {
      const paneIndex = existing.tab.panes.findIndex((pane) => pane.paneId === paneId);
      existing.tab.panes.splice(paneIndex, 1, recoveredPane);
      existing.tab.layout = setTerminalForPane(existing.tab.layout, paneId, paneId);
      continue;
    }
    tabs.value.push({
      tabId: createUuidV7(),
      layout: createTerminalPane(paneId, paneId),
      activePaneId: paneId,
      panes: [recoveredPane],
    });
  }
  for (const summary of activeTelnetSessions(telnetSnapshot)) {
    const paneId = telnetPaneBySessionId.get(summary.sessionId) ?? createUuidV7();
    const recoveredPane: TelnetSessionPane = {
      kind: "telnet",
      paneId,
      label: `${summary.endpoint.address}:${summary.endpoint.port}`,
      endpoint: summary.endpoint,
      state: summary.state,
      summary,
      deferredStart: false,
    };
    const existing = findWorkspacePane(paneId);
    if (existing) {
      const paneIndex = existing.tab.panes.findIndex((pane) => pane.paneId === paneId);
      existing.tab.panes.splice(paneIndex, 1, recoveredPane);
      existing.tab.layout = setTerminalForPane(existing.tab.layout, paneId, paneId);
      continue;
    }
    tabs.value.push({
      tabId: createUuidV7(),
      layout: createTerminalPane(paneId, paneId),
      activePaneId: paneId,
      panes: [recoveredPane],
    });
  }
  if (!activeTabId.value) activeTabId.value = tabs.value[0]?.tabId ?? "";
}

function restoreWorkspacePane(
  pane: PersistedTerminalWorkspacePane,
  hostsById: ReadonlyMap<string, HostSummary>,
  autoReconnectHistory: boolean,
): TerminalWorkspacePane {
  if (pane.kind === "launcher") return pane;
  if (pane.kind === "plugin") {
    return { kind: "plugin", paneId: pane.paneId, label: pane.label, profile: { pluginId: pane.pluginId, providerId: pane.providerId, schemaHash: pane.schemaHash, configuration: pane.configuration }, launch: null, state: "closed", summary: null, deferredStart: true };
  }
  if (pane.kind === "local") {
    return { ...pane, state: "starting", summary: null, deferredStart: false };
  }
  if (pane.kind === "telnet") {
    return {
      kind: "telnet",
      paneId: pane.paneId,
      label: pane.label,
      endpoint: { address: pane.address, port: pane.port },
      state: "closed",
      summary: null,
      // Every fresh Telnet connection requires a new explicit acknowledgement
      // of all cleartext and server-identity risks.
      deferredStart: true,
    };
  }
  if (pane.kind === "sshQuickConnect") {
    return {
      kind: "session",
      paneId: pane.paneId,
      label: pane.label,
      state: "closed",
      target: { kind: "quickConnect", endpoint: pane.endpoint },
      credentialRefId: null,
      summary: null,
      // Quick Connect credentials are intentionally never persisted. Keep the
      // endpoint visible, but require a fresh credential instead of retrying
      // without authentication material during startup.
      deferredStart: true,
      deferredRecovery: "credential",
    };
  }
  const host = hostsById.get(pane.hostId);
  if (!host) {
    return { kind: "launcher", paneId: pane.paneId, label: pane.label };
  }
  const canAutoReconnect = autoReconnectHistory && host.hasReadyCredential;
  const deferredRecovery = canAutoReconnect || !autoReconnectHistory
    ? "reconnect"
    : !host.hasReadyCredential
      ? "credential"
      : "reconnect";
  return {
    kind: "session",
    paneId: pane.paneId,
    label: pane.label,
    state: "closed",
    target: {
      kind: "host",
      hostId: host.hostId,
      expectedHostStateVersion: host.stateVersion,
    },
    credentialRefId: null,
    summary: null,
    deferredStart: !canAutoReconnect,
    deferredRecovery,
  };
}

function restorePersistedWorkspace(
  layout: PersistedTerminalWorkspaceLayout,
  hostsById: ReadonlyMap<string, HostSummary>,
  autoReconnectHistory: boolean,
) {
  tabs.value = layout.tabs.map((tab) => ({
    tabId: tab.tabId,
    layout: tab.layout,
    activePaneId: tab.activePaneId,
    panes: tab.panes.map((pane) => restoreWorkspacePane(
      pane,
      hostsById,
      autoReconnectHistory,
    )),
  }));
  activeTabId.value = layout.activeTabId ?? "";
}

async function persistOneWorkspaceProjection(serialized: string) {
  if (!workspacePersistenceEnabled || workspaceRevision === null
    || serialized === lastPersistedProjection) return;
  let parsed: PersistedTerminalWorkspaceLayout | null = null;
  try {
    parsed = parseTerminalWorkspaceLayout(JSON.parse(serialized));
  } catch {
    // The projection is generated locally, but the same fail-closed parser is
    // retained at this boundary so malformed state is never written.
  }
  if (!parsed) {
    workspacePersistenceEnabled = false;
    workspacePersistenceErrorVisible.value = true;
    return;
  }
  try {
    const snapshot = await replaceTerminalWorkspaceLayout({
      expectedRevision: workspaceRevision,
      layout: parsed,
    });
    workspaceRevision = snapshot.revision;
    lastPersistedProjection = serialized;
    workspacePersistenceErrorVisible.value = false;
  } catch {
    workspacePersistenceEnabled = false;
    workspacePersistenceErrorVisible.value = true;
  }
}

function persistWorkspaceProjection(serialized: string): Promise<void> {
  workspaceWriteChain = workspaceWriteChain.then(
    () => persistOneWorkspaceProjection(serialized),
  );
  return workspaceWriteChain;
}

function queueWorkspaceSave(serialized: string) {
  if (!workspaceInitialized || !workspacePersistenceEnabled || optimisticTabCloseInFlight) return;
  if (workspaceSaveTimer !== null) window.clearTimeout(workspaceSaveTimer);
  workspaceSaveTimer = window.setTimeout(() => {
    workspaceSaveTimer = null;
    void persistWorkspaceProjection(serialized);
  }, 400);
}

async function flushTerminalWorkspaceLayout() {
  if (optimisticTabCloseInFlight) {
    throw new Error("terminal tab closure is still pending");
  }
  while (true) {
    if (workspaceSaveTimer !== null) {
      window.clearTimeout(workspaceSaveTimer);
      workspaceSaveTimer = null;
    }
    const serialized = workspaceProjection.value;
    await persistWorkspaceProjection(serialized);
    if (serialized !== lastPersistedProjection) {
      throw new Error("terminal workspace layout is not durable");
    }
    if (workspaceProjection.value === serialized) return;
  }
}

watch(workspaceProjection, queueWorkspaceSave);

async function initializeTerminalWorkspace() {
  if (!canUseDesktopCore()) return;
  const [sshSnapshot, localSnapshot, telnetSnapshot, storedSnapshot, pluginSnapshot] = await Promise.all([
    fetchSshSessionSnapshot().catch(() => null),
    fetchLocalSessionSnapshot().catch(() => null),
    fetchTelnetSessionSnapshot().catch(() => null),
    fetchTerminalWorkspaceLayout().catch(() => null),
    fetchPluginTerminalSessionSnapshot().catch(() => null),
  ]);
  const persistedLayout = storedSnapshot
    ? parseTerminalWorkspaceLayout(storedSnapshot.layout)
    : null;
  if (storedSnapshot && persistedLayout) {
    workspaceRevision = storedSnapshot.revision;
    workspacePersistenceEnabled = true;
    lastPersistedProjection = JSON.stringify(persistedLayout);
  } else {
    workspacePersistenceErrorVisible.value = true;
  }

  // A failed Core snapshot is not evidence that no live session exists. Do
  // not mount the restart-only SQLite projection or overwrite it from an
  // uncertain renderer state; the next renderer launch may recover normally.
  if (!sshSnapshot || !localSnapshot || !telnetSnapshot || !pluginSnapshot) {
    workspacePersistenceEnabled = false;
    workspacePersistenceErrorVisible.value = true;
    workspaceInitialized = true;
    return;
  }

  const hasLiveSessions = activeSshSessions(sshSnapshot).length > 0
    || activeLocalSessions(localSnapshot).length > 0
    || activeTelnetSessions(telnetSnapshot).length > 0
    || pluginSnapshot.sessions.some((item) => item.cleanupBlocked || !["closed", "failed"].includes(item.state));
  const shouldRestoreHistory = ui.terminalStartupBehavior === "restoreHistory";
  const shouldAutoReconnectHistory = !hasLiveSessions && shouldRestoreHistory;
  let canRestorePersistedLayout = persistedLayout !== null;
  let hostsById = new Map<string, HostSummary>();
  if (persistedLayout && (hasLiveSessions || shouldRestoreHistory)) {
    const hasSavedHostPane = persistedLayout.tabs.some((tab) =>
      tab.panes.some((pane) => pane.kind === "sshHost"));
    if (hasSavedHostPane) {
      try {
        const hosts = await listHosts();
        hostsById = new Map(hosts.map((host) => [host.hostId, host]));
      } catch {
        workspacePersistenceEnabled = false;
        canRestorePersistedLayout = false;
        workspacePersistenceErrorVisible.value = true;
        if (!hasLiveSessions) {
          workspaceInitialized = true;
          return;
        }
      }
    }
  }

  if (hasLiveSessions) {
    const sshPaneBySessionId = new Map<string, string>();
    const localPaneBySessionId = new Map<string, string>();
    const telnetPaneBySessionId = new Map<string, string>();
    if (persistedLayout && canRestorePersistedLayout) {
      const persistedPaneIds = new Set(
        persistedLayout.tabs.flatMap((tab) => tab.panes.map((pane) => pane.paneId)),
      );
      await Promise.all([
        ...activeSshSessions(sshSnapshot).map(async (summary) => {
          const details = await getSshSession(summary.sessionId).catch(() => null);
          const attachment = details?.attachments.find((candidate) =>
            persistedPaneIds.has(candidate.viewId));
          if (attachment) sshPaneBySessionId.set(summary.sessionId, attachment.viewId);
        }),
        ...activeLocalSessions(localSnapshot).map(async (summary) => {
          const details = await getLocalSession(summary.sessionId).catch(() => null);
          const attachment = details?.attachments.find((candidate) =>
            persistedPaneIds.has(candidate.viewId));
          if (attachment) localPaneBySessionId.set(summary.sessionId, attachment.viewId);
        }),
      ]);
      const persistedTelnetPanes = persistedLayout.tabs
        .flatMap((tab) => tab.panes)
        .filter((pane): pane is Extract<PersistedTerminalWorkspacePane, { kind: "telnet" }> =>
          pane.kind === "telnet");
      const unused = new Set(persistedTelnetPanes.map((pane) => pane.paneId));
      for (const summary of activeTelnetSessions(telnetSnapshot)) {
        const match = persistedTelnetPanes.find((pane) => unused.has(pane.paneId)
          && pane.address === summary.endpoint.address
          && pane.port === summary.endpoint.port);
        if (match) {
          telnetPaneBySessionId.set(summary.sessionId, match.paneId);
          unused.delete(match.paneId);
        }
      }
    }
    if (persistedLayout && canRestorePersistedLayout) {
      // Publish the persisted tree and its live Pane replacements in one
      // synchronous phase. Vue must never mount a deferred component and then
      // reuse it for a late existingSession prop that setup did not attach.
      restorePersistedWorkspace(persistedLayout, hostsById, false);
    }
    recoverRendererSessions(
      sshSnapshot,
      localSnapshot,
      telnetSnapshot,
      sshPaneBySessionId,
      localPaneBySessionId,
      telnetPaneBySessionId,
    );
  } else if (persistedLayout && canRestorePersistedLayout && shouldRestoreHistory) {
    restorePersistedWorkspace(
      persistedLayout,
      hostsById,
      shouldAutoReconnectHistory,
    );
  }

  for (const summary of pluginSnapshot.sessions.filter((item) => item.cleanupBlocked || !["closed", "failed"].includes(item.state))) projectPluginSession(summary);
  workspaceInitialized = true;
  const currentProjection = workspaceProjection.value;
  if (
    currentProjection !== lastPersistedProjection
    && (hasLiveSessions || shouldRestoreHistory)
  ) {
    queueWorkspaceSave(currentProjection);
  }
  if (activeTabId.value) activateTab(activeTabId.value);
}

function ensureTerminalWorkspaceInitialized() {
  workspaceInitializationPromise ??= initializeTerminalWorkspace();
  return workspaceInitializationPromise;
}

const tabItems = computed(() => tabs.value.map((tab) => {
  const pane = tab.panes.find((candidate) => candidate.paneId === tab.activePaneId)
    ?? tab.panes[0]!;
  const state = pane.kind === "launcher"
    ? t("sshTerminal.newTabState")
    : pane.kind === "local"
      ? t(`localSession.states.${pane.state}`)
      : pane.kind === "plugin"
        ? t(`pluginTerminal.states.${pane.state}`)
        : pane.kind === "telnet"
        ? t(`telnetSession.states.${pane.state}`)
        : t(`sshSession.states.${pane.state}`);
  const bellAttention = tab.panes.some((item) => bellAttentionPaneIds.value.has(item.paneId));
  return {
    groupId: tab.tabId,
    label: pane.label,
    stateLabel: `${state} · ${t("sshTerminal.paneCount", { count: countTerminalPanes(tab.layout) })}`,
    hostId: pane.kind === "session" && pane.target.kind === "host" ? pane.target.hostId : null,
    bellAttention,
  };
}));


function focusExistingPane(matches: (pane: TerminalWorkspacePane) => boolean) {
  if (terminalFocusIsBlocked() || document.querySelector('[role="dialog"][aria-modal="true"]')) return false;
  const match = tabs.value.flatMap((tab) => tab.panes.map((pane) => ({ tab, pane }))).find(({ pane }) => matches(pane));
  if (!match) return false;
  activatePane(match.tab.tabId, match.pane.paneId);
  return true;
}


function focusNativeSession(scope: NativeTerminalSessionScope) {
  return focusExistingPane((pane) => {
    if (pane.paneId !== scope.paneId || (pane.kind !== "session" && pane.kind !== "local") || !pane.summary) return false;
    if (pane.summary.sessionId !== scope.sessionId || pane.summary.generation !== scope.generation) return false;
    return (scope.kind === "ssh" && pane.kind === "session" && pane.summary.channelId === scope.channelId)
      || (scope.kind === "local" && pane.kind === "local" && pane.summary.ptyId === scope.ptyId);
  });
}

function focusSshSession(sessionId: SshSessionId, generation: WireSequence) {
  return focusExistingPane((pane) => (
    pane.kind === "session"
      && pane.summary?.sessionId === sessionId
      && pane.summary.generation === generation
  ));
}

function focusLocalSession(sessionId: LocalSessionId, generation: WireSequence) {
  return focusExistingPane((pane) => (
    pane.kind === "local"
      && pane.summary?.sessionId === sessionId
      && pane.summary.generation === generation
  ));
}

function focusTelnetSession(sessionId: TelnetSessionId, generation: WireSequence, socketId: TelnetSocketId | null) {
  return focusExistingPane((pane) => (
    pane.kind === "telnet"
      && pane.summary?.sessionId === sessionId
      && pane.summary.generation === generation
      && pane.summary.socketId === socketId
  ));
}

watch(
  [tabItems, activeTabId, preparing, quickCommandsOpen],
  ([nextTabs, nextActiveTabId, nextBusy, nextQuickCommandsOpen]) => {
    workspaceTabs.syncTerminalState({
      tabs: nextTabs,
      activeTabId: nextActiveTabId,
      busy: nextBusy,
      activationBlocked: terminalFocusIsBlocked(),
      quickCommandsOpen: nextQuickCommandsOpen,
    });
  },
  { immediate: true, deep: true },
);

function splitActivePane(direction: TerminalSplitDirection) {
  const tab = activeTab.value;
  if (!tab || !canSplitActivePane(direction)) return;
  const paneId = createUuidV7();
  const nextLayout = splitTerminalPane(
    tab.layout,
    tab.activePaneId,
    direction,
    paneId,
    createUuidV7(),
  );
  if (nextLayout === tab.layout) return;
  tab.layout = nextLayout;
  tab.panes.push({ kind: "launcher", paneId, label: t("sshTerminal.newPaneLabel") });
  void refreshSavedHosts();
  activatePane(tab.tabId, paneId);
}

function splitWorkspaceRight() {
  const tab = activeTab.value;
  if (!tab || !workspaceRightSplitAllowed()) return;
  const paneId = createUuidV7();
  tab.layout = splitTerminalWorkspaceToRight(tab.layout, paneId, createUuidV7());
  tab.panes.push({ kind: "launcher", paneId, label: t("sshTerminal.newPaneLabel") });
  void refreshSavedHosts();
  activatePane(tab.tabId, paneId);
}

function workspaceRightSplitAllowed() {
  const tab = activeTab.value;
  if (!tab) return false;
  const activePaneElement = Array.from(
    document.querySelectorAll<HTMLElement>(".nvx-terminal-split-tree__pane--active"),
  ).find((element) => element.dataset.paneId === tab.activePaneId);
  const tree = activePaneElement?.closest<HTMLElement>(".nvx-terminal-split-tree");
  const available = tree?.getBoundingClientRect().width ?? 0;
  if (available <= 0) return true;
  const minimum = terminalLayoutMinimumSpanAfterWorkspaceRightSplit(tab.layout);
  return available >= minimum.widthUnits * 400;
}

function canSplitActivePane(direction: TerminalSplitDirection) {
  const tab = activeTab.value;
  if (!tab) return false;
  const activePaneElement = Array.from(
    document.querySelectorAll<HTMLElement>(".nvx-terminal-split-tree__pane--active"),
  ).find((element) => element.dataset.paneId === tab.activePaneId);
  const tree = activePaneElement?.closest<HTMLElement>(".nvx-terminal-split-tree");
  const bounds = tree?.getBoundingClientRect();
  const available = direction === "horizontal" ? bounds?.width ?? 0 : bounds?.height ?? 0;
  // During mount and in deterministic DOM tests, SplitTree treats a zero-sized
  // surface as unconstrained. Match that rule instead of rejecting a valid action.
  if (available <= 0) return true;
  const minimum = terminalLayoutMinimumSpanAfterSplit(tab.layout, tab.activePaneId, direction);
  const required = direction === "horizontal" ? minimum.widthUnits * 400 : minimum.heightUnits * 240;
  return available >= required;
}

function focusRelativePane(offset: 1 | -1) {
  const tab = activeTab.value;
  if (!tab || tab.panes.length < 2) return;
  const index = tab.panes.findIndex((pane) => pane.paneId === tab.activePaneId);
  const nextIndex = index < 0 ? 0 : (index + offset + tab.panes.length) % tab.panes.length;
  const pane = tab.panes[nextIndex];
  if (pane) activatePane(tab.tabId, pane.paneId);
}

function runTerminalShortcut(commandId: ShortcutCommandId) {
  switch (commandId) {
    case "workspace.new-local":
      createLocalTerminalTab();
      return;
    case "terminal.split-right":
      splitActivePane("horizontal");
      return;
    case "terminal.split-down":
      splitActivePane("vertical");
      return;
    case "terminal.focus-next-pane":
      focusRelativePane(1);
      return;
    case "terminal.focus-previous-pane":
      focusRelativePane(-1);
      return;
    case "terminal.close-pane": {
      const tab = activeTab.value;
      if (tab) requestClosePane(tab.tabId, tab.activePaneId);
      return;
    }
    default:
      if (activePane.value) paneRefs.get(activePane.value.paneId)?.runShortcut?.(commandId);
  }
}

function resizeSplit(tabId: string, splitId: string, ratio: number) {
  const tab = tabs.value.find((candidate) => candidate.tabId === tabId);
  if (tab) tab.layout = setTerminalSplitRatio(tab.layout, splitId, ratio);
}

async function openRequestedHost(
  hostId: string,
  paneId: string | null = null,
  options: {
    forceNewTab?: boolean;
    operationId?: string;
    pluginAuthorizationToken?: string;
    initialDirectory?: string | null;
  } = {},
) {
  const operationId = options.operationId;
  const inFlightKey = operationId ?? `host:${hostId}`;
  if (
    !hostId
    || !canUseDesktopCore()
    || consumingHostOperationId.value === inFlightKey
    || (operationId !== undefined && consumedHostOperationIds.has(operationId))
  ) return;
  consumingHostOperationId.value = inFlightKey;
  if (operationId !== undefined) consumedHostOperationIds.add(operationId);
  if (operationId !== undefined && consumedHostOperationIds.size > 64) {
    const oldest = consumedHostOperationIds.values().next().value;
    if (oldest) consumedHostOperationIds.delete(oldest);
  }
  try {
    launcherTargetPaneId.value = paneId
      ?? (options.forceNewTab
        ? createLauncherTab()
        : activeLauncherPane()?.paneId ?? createLauncherTab());
    const launchPaneId = launcherTargetPaneId.value;
    requestedInitialDirectory.value = options.initialDirectory
      ? { hostId, paneId: launchPaneId, directory: options.initialDirectory }
      : null;
    const host = (await listHosts()).find((candidate) => candidate.hostId === hostId);
    if (launcherTargetPaneId.value !== launchPaneId) {
      if (requestedInitialDirectory.value?.paneId === launchPaneId) requestedInitialDirectory.value = null;
      return;
    }
    if (!host) throw new Error("Requested Host no longer exists");
    requestedHost.value = host;
    requestedPluginAuthorizationToken.value = options.pluginAuthorizationToken ?? null;
    const state = await refreshVaultState();
    if (launcherTargetPaneId.value !== launchPaneId) {
      if (requestedInitialDirectory.value?.paneId === launchPaneId) requestedInitialDirectory.value = null;
      return;
    }
    if (host.hasReadyCredential) {
      if (state === "unlocked") {
        await connectSavedHost(host, options.pluginAuthorizationToken ?? null);
        await router.replace({ path: "/terminal" });
        return;
      }
      if (state === "locked" || state === "requiresReload") {
        openVaultPrompt("useSavedCredential", state);
        await router.replace({ path: "/terminal" });
        return;
      }
      if (state === null) {
        address.value = host.address;
        port.value = String(host.port);
        username.value = host.username ?? "";
        openLauncher();
        showVaultStatusUnavailable();
        await router.replace({ path: "/terminal" });
        return;
      }
    }
    address.value = host.address;
    port.value = String(host.port);
    username.value = host.username ?? "";
    openLauncher();
    await router.replace({ path: "/terminal" });
  } catch {
    if (requestedInitialDirectory.value?.hostId === hostId) requestedInitialDirectory.value = null;
    if (options.initialDirectory) {
      tips.show({ scope: "ssh-sftp-directory-launch", tone: "error", title: t("sshTerminal.sftpDirectoryConnectFailed") });
    }
    connectionErrorVisible.value = true;
  } finally {
    if (consumingHostOperationId.value === inFlightKey) consumingHostOperationId.value = null;
  }
}

async function consumeRequestedHostRoute() {
  const hostId = typeof route.query.hostId === "string" ? route.query.hostId : null;
  if (!hostId) return;
  const operationId = typeof route.query.connectOperationId === "string"
    ? route.query.connectOperationId
    : undefined;
  const sftpDirectoryLaunch = route.query.source === "sftpDirectory";
  const forceNewTab = route.query.source === "overview" || route.query.source === "plugin" || sftpDirectoryLaunch;
  const pluginAuthorizationToken = route.query.source === "plugin"
    && typeof route.query.pluginAuthorizationToken === "string"
    ? route.query.pluginAuthorizationToken
    : undefined;
  if (operationId !== undefined && consumedHostOperationIds.has(operationId)) {
    await router.replace({ path: "/terminal" });
    return;
  }
  const routeKey = operationId ?? `host:${hostId}`;
  if (consumingRouteOperationIds.has(routeKey)) return;
  consumingRouteOperationIds.add(routeKey);
  try {
    await ensureTerminalWorkspaceInitialized();
    const initialDirectory = sftpDirectoryLaunch && operationId
      ? takeSftpTerminalLaunch(operationId, hostId)
      : null;
    if (sftpDirectoryLaunch && (initialDirectory === null || !canUseDesktopCore())) {
      tips.show({ scope: "ssh-sftp-directory-launch", tone: "error", title: t("sshTerminal.sftpDirectoryLaunchExpired") });
      await router.replace({ path: "/terminal" });
      return;
    }
    await openRequestedHost(hostId, null, {
      forceNewTab,
      operationId,
      pluginAuthorizationToken,
      initialDirectory,
    });
  } catch {
    if (sftpDirectoryLaunch) {
      tips.show({ scope: "ssh-sftp-directory-launch", tone: "error", title: t("sshTerminal.sftpDirectoryConnectFailed") });
    } else {
      connectionErrorVisible.value = true;
    }
    await router.replace({ path: "/terminal" });
  } finally {
    consumingRouteOperationIds.delete(routeKey);
  }
}

async function consumeRequestedSessionFocus() {
  const sessionId = typeof route.query.focusSessionId === "string"
    ? route.query.focusSessionId
    : null;
  if (!sessionId) return;
  await ensureTerminalWorkspaceInitialized();
  const match = tabs.value.flatMap((tab) => tab.panes.map((pane) => ({ tab, pane })))
    .find(({ pane }) => pane.kind === "session" && pane.summary?.sessionId === sessionId);
  if (match) activatePane(match.tab.tabId, match.pane.paneId);
  await router.replace({ path: "/terminal" });
}

watch(
  () => [
    typeof route.query.hostId === "string" ? route.query.hostId : null,
    typeof route.query.connectOperationId === "string" ? route.query.connectOperationId : null,
  ],
  () => void consumeRequestedHostRoute(),
  { immediate: true },
);

watch(
  () => typeof route.query.focusSessionId === "string" ? route.query.focusSessionId : null,
  () => void consumeRequestedSessionFocus(),
  { immediate: true },
);

watch(
  [launcherOpen, telnetLauncherOpen, vaultDialogOpen, closeCandidate],
  () => {
    workspaceTabs.syncTerminalState({
      tabs: tabItems.value,
      activeTabId: activeTabId.value,
      busy: preparing.value,
      activationBlocked: terminalFocusIsBlocked(),
      quickCommandsOpen: quickCommandsOpen.value,
    });
    if (terminalFocusIsBlocked()) {
      void clearTerminalInputFocus();
      return;
    }
    const tab = activeTab.value;
    const pane = tab?.panes.find((candidate) => candidate.paneId === tab.activePaneId);
    if (pane && pane.kind !== "launcher") paneRefs.get(pane.paneId)?.activateFromTab();
  },
);

// KeepAlive pauses the route view while Hosts is active. Re-consume the
// explicit Host intent after activation so navigation can never become a
// silent route-only change.
const revealRoute = useRouteReveal();
let initialRouteReady = false;
onActivated(() => {
  void reconcilePluginProtocolLaunches();
  void refreshSavedHosts();
  void consumeRequestedHostRoute();
  void consumeRequestedSessionFocus();
  reconcileTerminalAfterForeground();
  if (initialRouteReady) revealRoute();
});

onBeforeUnmount(() => {
  terminalViewDisposed = true;
  unlistenSavedConnections?.();
  vaultPromptMode.value = null;
  unlistenPluginProtocolLaunch?.();
  unlistenPluginProtocolLaunch = null;
  unlistenPluginTerminalChannel?.();
  unlistenPluginTerminalChannel = null;
  window.removeEventListener("focus", reconcileTerminalAfterForeground);
  document.removeEventListener("visibilitychange", handleTerminalVisibilityChange);
  unregisterTerminalHeaderController?.();
  unregisterTerminalHeaderController = null;
  clearCloseTerminalsTimeout();
  unregisterWorkspaceFlush?.();
  unregisterWorkspaceFlush = null;
  // Route teardown keeps the user-visible persistence warning, but only the
  // native exit barrier treats a failed durable write as an exit blocker.
  void flushTerminalWorkspaceLayout().catch(() => undefined);
});

onMounted(async () => {
  void onSavedConnectionsChanged(() => { void refreshSavedHosts(); }).then((unlisten) => {
    if (terminalViewDisposed) unlisten(); else unlistenSavedConnections = unlisten;
  }).catch(() => { /* Activation also refreshes saved Hosts. */ });
  window.addEventListener("focus", reconcileTerminalAfterForeground);
  document.addEventListener("visibilitychange", handleTerminalVisibilityChange);
  unregisterWorkspaceFlush = registerTerminalWorkspaceFlush(flushTerminalWorkspaceLayout);
  try {
    await Promise.all([ensureTerminalWorkspaceInitialized(), refreshSavedHosts()]);
    unregisterTerminalHeaderController = workspaceTabs.registerTerminalController({
      activate: activateTab,
      close: requestCloseTab,
      closeMany: requestCloseTabs,
      create: createNewTerminalTab,
      createLocal: createLocalFromHeader,
      quickConnect: quickConnectFromHeader,
      deactivate: deactivateTerminalWorkspace,
      toggleQuickCommands: () => { quickCommandsOpen.value = !quickCommandsOpen.value; },
      runShortcut: runTerminalShortcut,
      focusNativeSession,
      focusSshSession,
      focusLocalSession,
      focusTelnetSession,
    });
  } finally {
    initialRouteReady = true;
    revealRoute();
  }
  if (canUseDesktopCore()) {
    unlistenPluginProtocolLaunch = await listen("plugin-protocol-launch", () => { void reconcilePluginProtocolLaunches(); });
    await reconcilePluginProtocolLaunches();
    unlistenPluginTerminalChannel = await listen<PluginApprovedTerminalChannelLaunch>("plugin-terminal-channel-approved", ({ payload }) => {
      if (approvedPluginChannelOperations.has(payload.operationId)) return;
      approvedPluginChannelOperations.add(payload.operationId);
      const paneId = createLauncherTab(false);
      replaceWorkspacePane(paneId, {
        kind: "session", paneId, label: payload.label, target: payload.target,
        credentialRefId: null, pluginAuthorizationToken: payload.authorizationToken,
        state: "connecting", summary: null, deferredStart: false, deferredRecovery: "reconnect",
      });
    });
  }
});
</script>

<template>
  <div class="ssh-terminal-page">
    <NvxTerminalPluginRegion
      class="ssh-terminal-page__plugin-header"
      region="header"
      :instance-key="activePluginContextKey"
      :context-label="activePluginContextLabel"
      :available="activePluginSessionAvailable"
    />
    <section
      class="ssh-terminal-page__surface"
      :aria-label="t('navigation.terminal')"
    >
      <NvxInlineNotice
        v-if="workspacePersistenceErrorVisible"
        class="terminal-workspace__persistence-error"
        tone="warning"
        :title="t('sshTerminal.workspacePersistenceFailed')"
      />
      <NvxInlineNotice
        v-if="pluginLaunchErrorVisible"
        tone="warning"
        :title="t('pluginTerminal.launchRecoveryFailed')"
      >
        <NvxButton
          size="sm"
          variant="ghost"
          @click="reconcilePluginProtocolLaunches"
        >
          {{ t('pluginTerminal.retry') }}
        </NvxButton>
      </NvxInlineNotice>
      <div
        v-if="!tabs.length"
        class="ssh-terminal-empty"
      >
        <NvxTerminalLauncher
          :hosts="launcherHosts"
          @quick-connect="openInlineQuickConnect($event)"
          @connect-host="openRequestedHost($event)"
          @add-host="openHosts"
          @local-terminal="createLocalTerminal(null)"
          @import-ssh-config="openSshConfigImport"
          @telnet="openTelnetLauncher(null)"
          @show-all-hosts="showAllHosts"
        />
      </div>
      <template
        v-for="tab in tabs"
        :key="tab.tabId"
      >
        <section
          v-show="tab.tabId === activeTabId"
          class="terminal-workspace"
        >
          <NvxTerminalSplitTree
            :node="tab.layout"
            :active-pane-id="tab.activePaneId"
            :separator-label="t('sshTerminal.resizeSeparator')"
            @activate="activatePane(tab.tabId, $event)"
            @resize="(splitId, ratio) => resizeSplit(tab.tabId, splitId, ratio)"
          >
            <template #pane="{ pane: layoutPane, canSplitHorizontal, canSplitVertical, canSplitWorkspaceRight }">
              <template
                v-for="pane in tab.panes"
                :key="pane.paneId"
              >
                <section
                  v-if="pane.paneId === layoutPane.paneId && pane.kind === 'launcher'"
                  class="terminal-pane-launcher"
                >
                  <header class="terminal-pane-launcher__toolbar">
                    <span>{{ pane.label }}</span>
                    <NvxTerminalPaneControls
                      v-if="tab.tabId === activeTabId && tab.activePaneId === pane.paneId"
                      :plugin-context-key="pane.paneId"
                      :can-split-horizontal="canSplitHorizontal"
                      :can-split-vertical="canSplitVertical"
                      :can-split-workspace-right="canSplitWorkspaceRight"
                      :show-layout-actions="true"
                      @split="splitActivePane"
                      @split-workspace-right="splitWorkspaceRight"
                      @close="requestClosePane(tab.tabId, pane.paneId)"
                    />
                  </header>
                  <div class="terminal-pane-launcher__body">
                    <NvxTerminalLauncher
                      :hosts="launcherHosts"
                      @quick-connect="openInlineQuickConnect($event, pane.paneId)"
                      @connect-host="openRequestedHost($event, pane.paneId)"
                      @add-host="openHosts"
                      @local-terminal="createLocalTerminal(pane.paneId)"
                      @import-ssh-config="openSshConfigImport"
                      @telnet="openTelnetLauncher(pane.paneId)"
                      @show-all-hosts="showAllHosts"
                    />
                  </div>
                </section>
                <NvxSshTerminalPane
                  v-else-if="pane.paneId === layoutPane.paneId && pane.kind === 'session'"
                  :ref="(value) => setPaneRef(pane.paneId, value)"
                  :pane-id="pane.paneId"
                  :label="pane.label"
                  :target="pane.target"
                  :credential-ref-id="pane.credentialRefId"
                  :plugin-authorization-token="pane.pluginAuthorizationToken ?? null"
                  :existing-session="pane.summary"
                  :deferred-start="pane.deferredStart"
                  :deferred-recovery="pane.deferredRecovery"
                  :initial-directory="pane.initialDirectory ?? null"
                  :active="route.path === '/terminal' && tab.tabId === activeTabId && tab.activePaneId === pane.paneId"
                  :visible="route.path === '/terminal' && tab.tabId === activeTabId"
                  :can-split-horizontal="canSplitHorizontal"
                  :can-split-vertical="canSplitVertical"
                  :can-split-workspace-right="canSplitWorkspaceRight"
                  @activate="activatePane(tab.tabId, pane.paneId)"
                  @state="(state, summary) => updateTabState(pane.paneId, state, summary)"
                  @bell-attention="setPaneBellAttention(pane.paneId, $event)"
                  @request-authentication-recovery="(_, target) => requestSessionAuthenticationRecovery(pane.paneId, target)"
                  @request-credential="(_, target) => requestSessionRecovery(pane.paneId, target, 'credential')"
                  @request-vault-unlock="(_, target) => requestSessionRecovery(pane.paneId, target, 'vault')"
                  @initial-directory-handled="(directory) => { if (pane.initialDirectory === directory) pane.initialDirectory = null; }"
                  @split="splitActivePane"
                  @split-workspace-right="splitWorkspaceRight"
                  @close="requestClosePane(tab.tabId, pane.paneId)"
                />
                <NvxLocalTerminalPane
                  v-else-if="pane.paneId === layoutPane.paneId && pane.kind === 'local'"
                  :ref="(value) => setPaneRef(pane.paneId, value)"
                  :pane-id="pane.paneId"
                  :label="pane.label"
                  :existing-session="pane.summary"
                  :deferred-start="pane.deferredStart"
                  :active="route.path === '/terminal' && tab.tabId === activeTabId && tab.activePaneId === pane.paneId"
                  :visible="route.path === '/terminal' && tab.tabId === activeTabId"
                  :can-split-horizontal="canSplitHorizontal"
                  :can-split-vertical="canSplitVertical"
                  :can-split-workspace-right="canSplitWorkspaceRight"
                  @activate="activatePane(tab.tabId, pane.paneId)"
                  @state="(state, summary) => updateLocalTabState(pane.paneId, state, summary)"
                  @bell-attention="setPaneBellAttention(pane.paneId, $event)"
                  @split="splitActivePane"
                  @split-workspace-right="splitWorkspaceRight"
                  @close="requestClosePane(tab.tabId, pane.paneId)"
                />
                <NvxPluginTerminalPane
                  v-else-if="pane.paneId === layoutPane.paneId && pane.kind === 'plugin'"
                  :ref="(value) => setPaneRef(pane.paneId, value)"
                  :pane-id="pane.paneId"
                  :tab-id="tab.tabId"
                  :label="pane.label"
                  :profile="pane.profile"
                  :launch="pane.launch"
                  :existing-session="pane.summary"
                  :deferred-start="pane.deferredStart"
                  :active="route.path === '/terminal' && tab.tabId === activeTabId && tab.activePaneId === pane.paneId"
                  :can-split-horizontal="canSplitHorizontal"
                  :can-split-vertical="canSplitVertical"
                  :can-split-workspace-right="canSplitWorkspaceRight"
                  @activate="activatePane(tab.tabId, pane.paneId)"
                  @state="(state, summary) => updatePluginTabState(pane.paneId, state, summary)"
                  @bell-attention="setPaneBellAttention(pane.paneId, $event)"
                  @split="splitActivePane"
                  @split-workspace-right="splitWorkspaceRight"
                  @close="requestClosePane(tab.tabId, pane.paneId)"
                />
                <NvxTelnetTerminalPane
                  v-else-if="pane.paneId === layoutPane.paneId && pane.kind === 'telnet'"
                  :ref="(value) => setPaneRef(pane.paneId, value)"
                  :pane-id="pane.paneId"
                  :label="pane.label"
                  :endpoint="pane.endpoint"
                  :existing-session="pane.summary"
                  :deferred-start="pane.deferredStart"
                  :active="tab.tabId === activeTabId && tab.activePaneId === pane.paneId"
                  :can-split-horizontal="canSplitHorizontal"
                  :can-split-vertical="canSplitVertical"
                  :can-split-workspace-right="canSplitWorkspaceRight"
                  @activate="activatePane(tab.tabId, pane.paneId)"
                  @state="(state, summary) => updateTelnetTabState(pane.paneId, state, summary)"
                  @bell-attention="setPaneBellAttention(pane.paneId, $event)"
                  @split="splitActivePane"
                  @split-workspace-right="splitWorkspaceRight"
                  @close="requestClosePane(tab.tabId, pane.paneId)"
                />
              </template>
            </template>
          </NvxTerminalSplitTree>
        </section>
      </template>
    </section>

    <aside
      v-show="quickCommandsOpen || pluginToolsOpen || embeddedSidebarCount > 0"
      class="ssh-terminal-page__tools"
    >
      <Transition name="quick-commands">
        <NvxQuickCommandsSidebar v-if="quickCommandsOpen" />
      </Transition>
      <NvxPluginToolPanel
        v-model="pluginToolsOpen"
        target-id="terminal.tools"
        :instance-key="activePluginContextKey"
        :context-label="activePluginContextLabel"
        :available="activePluginSessionAvailable"
        @catalog="terminalToolCatalogCount = $event.length"
      />
      <NvxTerminalPluginRegion
        region="sidebar"
        :instance-key="activePluginContextKey"
        :context-label="activePluginContextLabel"
        :available="activePluginSessionAvailable"
        @availability="embeddedSidebarCount = $event"
      />
    </aside>
    <NvxTerminalPluginRegion
      class="ssh-terminal-page__plugin-footer"
      region="footer"
      :instance-key="activePluginContextKey"
      :context-label="activePluginContextLabel"
      :available="activePluginSessionAvailable"
    />
    <NvxPluginFloatingControls
      target-id="terminal.floating"
      :instance-key="activePluginContextKey"
      :context-label="activePluginContextLabel"
      :available="activePluginSessionAvailable"
      preference-scope="terminal"
      route-path="/terminal"
    />

    <NvxDialog
      :model-value="closeConfirmationVisible"
      :title="closeCandidate?.mode === 'pane'
        ? t('sshTerminal.closeActivePaneTitle')
        : closeCandidate?.mode === 'tabs'
          ? t('sshTerminal.closeMultipleTabsTitle')
          : t('sshTerminal.closeActiveTabTitle')"
      :description="closeCandidate?.mode === 'pane'
        ? t('sshTerminal.closeActivePaneBody')
        : closeCandidate?.mode === 'tabs'
          ? t('sshTerminal.closeMultipleTabsBody', {
            count: closeCandidateSessionCount,
            tabs: closeCandidate.tabIds.length,
          })
          : t('sshTerminal.closeActiveTabBody', { count: closeCandidateSessionCount })"
      :close-label="t('sshTerminal.cancel')"
      :dismissible="!closingTab && !closePreferenceSaving"
      @update:model-value="(open) => { if (!open) cancelCloseTab(); }"
    >
      <NvxInlineNotice
        v-if="closeTabErrorVisible"
        tone="error"
        :title="t('sshTerminal.closeTerminalFailed')"
      />
      <NvxCheckbox
        v-if="closeCandidateCanDisablePrompt"
        v-model="skipFutureSinglePaneTabClosePrompt"
        :disabled="closingTab || closePreferenceSaving"
      >
        {{ t("sshTerminal.doNotAskAgainForSinglePaneTab") }}
        <template #hint>
          {{ t("sshTerminal.multiPaneTabsAlwaysConfirm") }}
        </template>
      </NvxCheckbox>
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="closingTab || closePreferenceSaving"
          @click="cancelCloseTab"
        >
          {{ t("sshTerminal.cancel") }}
        </NvxButton>
        <NvxButton
          :loading="closingTab || closePreferenceSaving"
          @click="confirmCloseTab"
        >
          {{ closeCandidate?.mode === "pane"
            ? t("sshTerminal.disconnectAndClosePane")
            : closeCandidate?.mode === "tabs"
              ? t("sshTerminal.disconnectSessionsAndCloseTabs", {
                count: closeCandidateSessionCount,
                tabs: closeCandidate.tabIds.length,
              })
              : t("sshTerminal.disconnectSessionsAndClose", { count: closeCandidateSessionCount }) }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      :model-value="telnetLauncherOpen"
      :title="t('telnetSession.dialogTitle')"
      :description="t('telnetSession.dialogDescription')"
      :close-label="t('telnetSession.cancel')"
      @update:model-value="(open) => { if (!open) closeTelnetLauncher(); }"
    >
      <NvxInlineNotice
        v-if="telnetValidationVisible"
        tone="error"
        :title="t('telnetSession.validationFailed')"
      />
      <NvxField
        for-id="telnet-address"
        :label="t('telnetSession.address')"
      >
        <NvxInput
          id="telnet-address"
          v-model="telnetAddress"
          :placeholder="t('telnetSession.addressPlaceholder')"
          data-nvx-dialog-initial-focus
        />
      </NvxField>
      <NvxField
        for-id="telnet-port"
        :label="t('telnetSession.port')"
      >
        <NvxInput
          id="telnet-port"
          v-model="telnetPort"
          inputmode="numeric"
        />
      </NvxField>
      <NvxInlineNotice
        tone="warning"
        :title="t('telnetSession.riskTitle')"
      >
        <p>{{ t("telnetSession.riskDescription") }}</p>
      </NvxInlineNotice>
      <NvxCheckbox v-model="telnetAcceptsCleartext">
        {{ t("telnetSession.acceptCleartext") }}
      </NvxCheckbox>
      <NvxCheckbox v-model="telnetAcceptsMissingIdentity">
        {{ t("telnetSession.acceptMissingIdentity") }}
      </NvxCheckbox>
      <NvxCheckbox v-model="telnetAcceptsTampering">
        {{ t("telnetSession.acceptTampering") }}
      </NvxCheckbox>
      <template #actions>
        <NvxButton
          variant="ghost"
          @click="closeTelnetLauncher"
        >
          {{ t("telnetSession.cancel") }}
        </NvxButton>
        <NvxButton @click="createTelnetTerminal">
          {{ t("telnetSession.connect") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      :model-value="launcherOpen"
      :dismissible="!preparing"
      plugin-protected
      :title="reauthenticating ? t('sshTerminal.reauthenticateTitle') : t('sshTerminal.dialogTitle')"
      :description="reauthenticating ? t('sshTerminal.reauthenticateDescription') : t('sshTerminal.dialogDescription')"
      :close-label="t('sshTerminal.closeDialog')"
      @update:model-value="(open) => { if (open) launcherOpen = true; else closeLauncher(); }"
    >
      <NvxField
        for-id="quick-address"
        :label="t('sshTerminal.address')"
      >
        <NvxInput
          id="quick-address"
          v-model="address"
          :disabled="reauthenticating || preparing"
          :placeholder="t('sshTerminal.addressPlaceholder')"
          data-nvx-dialog-initial-focus
        />
      </NvxField>
      <div class="ssh-terminal-dialog__row">
        <NvxField
          for-id="quick-port"
          :label="t('sshTerminal.port')"
        >
          <NvxInput
            id="quick-port"
            v-model="port"
            :disabled="reauthenticating || preparing"
            placeholder="22"
          />
        </NvxField>
        <NvxField
          for-id="quick-username"
          :label="t('sshTerminal.username')"
        >
          <NvxInput
            id="quick-username"
            v-model="username"
            :disabled="reauthenticating || preparing"
            :placeholder="t('sshTerminal.usernamePlaceholder')"
          />
        </NvxField>
      </div>
      <NvxField
        for-id="quick-auth"
        :label="t('sshTerminal.authMethod')"
      >
        <NvxSelect
          id="quick-auth"
          v-model="authentication"
          :disabled="preparing"
          :options="authOptions"
        />
      </NvxField>
      <NvxCheckbox
        id="quick-save-credential"
        v-model="saveCredential"
        :disabled="preparing"
      >
        {{ t("sshTerminal.saveCredential") }}
        <template #hint>
          {{ t("sshTerminal.saveCredentialHint") }}
        </template>
      </NvxCheckbox>
      <NvxInlineNotice
        v-if="validationVisible"
        tone="warning"
        :title="t('sshTerminal.invalidEndpoint')"
      />
      <NvxInlineNotice
        v-if="connectionErrorVisible"
        tone="error"
        :title="t(connectionErrorKey)"
      >
        <template v-if="connectionErrorBodyKey">
          {{ t(connectionErrorBodyKey) }}
        </template>
      </NvxInlineNotice>
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="preparing"
          @click="closeLauncher"
        >
          {{ t("sshTerminal.cancel") }}
        </NvxButton>
        <NvxButton
          :loading="preparing"
          @click="attemptConnect"
        >
          {{ t("sshTerminal.connect") }}
        </NvxButton>
      </template>
    </NvxDialog>
  </div>
</template>

<style scoped>
.ssh-terminal-page {
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  grid-template-rows: auto minmax(0, 1fr) auto;
  height: 100%;
  min-height: 100%;
  overflow: hidden;
  background: var(--nvx-color-bg-canvas);
}
.ssh-terminal-page__plugin-header { grid-column: 1 / -1; grid-row: 1; }
.ssh-terminal-page__plugin-footer { grid-column: 1 / -1; grid-row: 3; }
.ssh-terminal-page__tools { display: flex; grid-column: 2; grid-row: 2; flex-direction: column; min-width: 0; min-height: 0; height: 100%; }
.ssh-terminal-page__tools > :deep(.plugin-tool-panel) { flex: 1; min-height: 0; }

@media (max-width: 1100px) {
  .ssh-terminal-page {
    grid-template-columns: minmax(0, 1fr);
  }
  .ssh-terminal-page__tools { position: absolute; grid-column: 1 / -1; grid-row: 2; inset: 0; width: 100%; z-index: var(--nvx-z-popover); pointer-events: none; }
  .ssh-terminal-page__tools > :deep(*) { pointer-events: auto; }
  .ssh-terminal-page__tools > :deep(.terminal-plugin-region--sidebar) { position: absolute; right: 0; bottom: 0; width: min(340px, calc(100% - 24px)); }
}

.quick-commands-enter-active,
.quick-commands-leave-active {
  overflow: hidden;
  transition:
    width var(--nvx-motion-overlay) ease,
    opacity var(--nvx-motion-fast) ease,
    transform var(--nvx-motion-overlay) ease;
}

.quick-commands-enter-from,
.quick-commands-leave-to {
  width: 0;
  opacity: 0;
  transform: translateX(var(--nvx-space-2));
}

@media (prefers-reduced-motion: reduce) {
  .quick-commands-enter-active,
  .quick-commands-leave-active {
    transition: none;
  }
}

.ssh-terminal-page__surface {
  grid-column: 1;
  grid-row: 2;
  position: relative;
  display: grid;
  container-type: inline-size;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  place-items: start center;
  padding: 0;
}

.terminal-workspace {
  position: relative;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}

.terminal-workspace__persistence-error {
  position: absolute;
  z-index: 4;
  top: var(--nvx-space-3);
  left: 50%;
  width: min(560px, calc(100% - var(--nvx-space-6)));
  transform: translateX(-50%);
}

.terminal-pane-launcher {
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  background: var(--nvx-color-bg-canvas);
  color: var(--nvx-color-text-primary);
}

.terminal-pane-launcher__toolbar {
  display: flex;
  min-height: 36px;
  align-items: center;
  justify-content: space-between;
  padding: 0 var(--nvx-space-2);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  color: var(--nvx-color-text-secondary);
  font-family: var(--nvx-font-mono);
  font-size: var(--nvx-font-size-xs);
}

.terminal-pane-launcher__body {
  display: grid;
  container-type: inline-size;
  align-content: start;
  justify-items: center;
  min-width: 0;
  min-height: 0;
  padding: var(--nvx-space-8);
  overflow: auto;
}

.ssh-terminal-empty {
  box-sizing: border-box;
  display: grid;
  container-type: inline-size;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  align-content: start;
  justify-items: center;
  overflow: auto;
  padding: var(--nvx-space-10) var(--nvx-space-8) var(--nvx-space-8);
}

.ssh-terminal-dialog__row {
  display: grid;
  grid-template-columns: 120px 1fr;
  gap: var(--nvx-space-3);
}
</style>
