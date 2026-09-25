<script setup lang="ts">
import { ensureHostVault } from "../core-api/secure-vault-client";
import { openToolWindow, onToolWindowChanged } from "../tool-windows";
import { takeNativeTransferNavigation, matchesNativeTransferNavigation } from "../native-transfer-navigation";
import { homeDir, sep } from "@tauri-apps/api/path";
import { getCurrentWebview, type DragDropEvent } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Activity, Archive, ArrowUp, Download, Eye, FileArchive, FilePlus2, FolderOpen, FolderPlus, HardDrive, Link2, ListFilter, Pause, Pencil, PlugZap, Radio, RefreshCw, RotateCcw, Save, Search, Server, ShieldCheck, Square, Trash2, X } from "lucide-vue-next";
import { computed, nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import { sftpEntryIcon } from "../components/sftp/fileIcons";
import NvxSftpPaneActionsMenu from "../components/sftp/NvxSftpPaneActionsMenu.vue";
import { NvxPluginExtensionTarget } from "../components/plugins";
import {
  NvxTerminalSplitTree, closeTerminalPane, countTerminalPanes, createTerminalPane,
  findTerminalPane, setTerminalForPane, setTerminalSplitRatio, splitTerminalPane,
  type TerminalLayoutNode, type TerminalPaneNode, type TerminalSplitDirection,
} from "../components/terminal";
import { NvxButton, NvxCheckbox, NvxCodeEditor, NvxDialog, NvxField, NvxIcon, NvxIconButton, NvxInlineNotice, NvxInput, NvxProgress, NvxSelect, NvxStatusLabel } from "../components/ui";
import {
  cancelSftpDirectoryListing, cancelSftpTransfer, cancelSftpTransferIntent, canUseDesktopCore, disconnectSftpSession,
  createSftpLocalDirectoryChild, enqueueSftpTransfer, enqueueSftpTransferIntent, fetchSftpSessionSnapshot, fetchSftpTransferIntentSnapshot,
  fetchSshSessionSnapshot,
  listHosts, listSftpDirectory, listSftpLocalDirectory, mutateSftpFile, openSftpLocalDirectoryChild, parseCoreApiError,
  openSftpSession, prepareSftpTransferIntent, previewSftpFile, registerSftpLocalBoundary, registerSftpLocalDirectory, tailSftpFile,
  releaseSftpLocalDirectory, resumeSftpTransfer, retainSftpRemoteCleanupForExit,
  retainSftpTransferIntentCleanupForExit, retrySftpRemoteCleanup, retrySftpTransferIntentCleanup,
} from "../core-api/client";
import type {
  HostSummary, SftpConflictPolicy, SftpFailureCode, SftpFilePreviewContent, SftpLocalDirectoryCapability, SftpSessionState, SftpTextLineEnding,
  SftpSessionSummary, SftpTransferIntentSummary, SftpTransferState, SftpTransferSummary,
} from "../core-api/generated/core-api";
import { useSftpPreferencesStore } from "../stores/sftpPreferences";
import { useTipsStore } from "../stores/tips";
import {
  beginSftpPaneDirectoryLoad, captureSftpPaneDragIntent, cloneSftpPaneState,
  completeSftpPaneDirectoryLoad, createSftpPaneState, failSftpPaneDirectoryLoad,
  reconcileSftpPaneRemoteGeneration, replaceSftpPaneEndpoint, resolveSftpPaneDropFence,
  selectSftpPaneEntry, selectSftpPaneEntryRange, toggleSftpPaneEntry,
  visibleSftpPaneEntries, type SftpPaneDragIntent, type SftpPaneEntry, type SftpPaneState,
} from "./sftpPaneState";
import { pendingSftpPluginNavigations, resolveSftpPluginNavigation, takeSftpPluginNavigation } from "./sftpPluginNavigation";

interface LocalTrailItem {
  capability: SftpLocalDirectoryCapability;
  displayPath: string;
  rememberedPath: string | null;
}
interface RecursiveDirectoryEndpoint {
  kind: "local" | "remote";
  directoryRef: string;
  revision?: string;
  sessionId?: string;
  generation?: string;
  pathBytes?: number[];
}
interface RecursiveEntry {
  entryRef: string;
  displayName: string;
  kind: SftpPaneEntry["kind"];
  size: number | null;
  remotePathBytes: number[] | null;
}
interface RecursiveCopyBudget { entries: number; bytes: number }
type OverwriteDecision = "skip" | "replaceOnce" | "replaceAll";
interface OverwritePromptState {
  displayName: string;
  kind: "file" | "directory";
  allowReplaceAll: boolean;
  resolve: (decision: OverwriteDecision) => void;
}
interface PermissionsTarget {
  paneId: string;
  endpointRevision: number;
  directoryRevision: number;
  sessionId: string;
  generation: string;
  entryKey: string;
  displayName: string;
  pathBytes: number[];
  precondition: SftpPaneEntry["precondition"];
  expectedPermissionBits: number;
}
interface PointerDragState {
  pointerId: number;
  sourcePaneId: string;
  entryKey: string;
  originX: number;
  originY: number;
  intent: SftpPaneDragIntent | null;
}
interface TransferTargetRefresh {
  paneId: string;
  endpointRevision: number;
  directoryRevision: number;
}
interface SftpContextMenuState {
  anchorX: number;
  anchorY: number;
  paneId: string;
  left: number;
  top: number;
}
interface TextPreviewTarget {
  paneId: string;
  endpointRevision: number;
  directoryRevision: number;
  selectionRevision: number;
  sessionId: string;
  generation: string;
  directoryRef: string;
  entryRef: string;
  entryKey: string;
  pathBytes: number[];
  precondition: SftpPaneEntry["precondition"];
}
const { t, locale } = useI18n();
const router = useRouter();
const tips = useTipsStore();
const browserPreferences = useSftpPreferencesStore();
const operationFeedbackScope = "sftp-operation";
const navigationFeedbackScope = "sftp-plugin-navigation";
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const localPaneId = "sftp-local-pane";
const remotePaneId = "sftp-remote-pane";
const maximumRecursiveCopyDepth = 32;
const maximumRecursiveCopyEntries = 10_000;
const maximumRecursiveCopyBytes = 20 * 1024 * 1024 * 1024;
const maximumTailEditorCharacters = 2 * 1024 * 1024;
const pointerDragThreshold = 6;
const previewImageExtensions = new Set(["png", "jpg", "jpeg", "gif", "webp"]);
const nginxSiteDirectoryPrefixes = [
  encoder.encode("/etc/nginx/sites-available/"),
  encoder.encode("/etc/nginx/sites-enabled/"),
];
const previewTextExtensions = new Set([
  "txt", "md", "markdown", "log", "json", "jsonl", "yaml", "yml", "toml", "ini", "conf", "cfg", "xml",
  "csv", "tsv", "sh", "bash", "zsh", "fish", "ps1", "js", "mjs", "cjs", "ts", "tsx", "jsx", "vue",
  "css", "scss", "less", "html", "htm", "svg", "rs", "py", "rb", "go", "java", "c", "h", "cpp", "hpp",
  "swift", "kt", "sql", "env", "gitignore",
]);
const previewTextNames = new Set(["dockerfile", "makefile", "license", "readme", "hosts", "known_hosts"]);
const hosts = ref<HostSummary[]>([]);
const sessions = ref<SftpSessionSummary[]>([]);
const legacyTransfers = ref<SftpTransferSummary[]>([]);
const intentTransfers = ref<SftpTransferIntentSummary[]>([]);
const loading = ref(true);
const operationPending = ref(false);
const parentSessionLabels = reactive<Record<string, string>>({});
let navigationReady = false;
let navigationWorking = false;
const transfersOpen = ref(false);
const transferActivityRoot = ref<HTMLElement | null>(null);
const mutationDialog = ref<"mkdir" | "touch" | "rename" | "delete" | null>(null);
const mutationTargetPaneId = ref<string | null>(null);
const mutationName = ref("");
const fileUtilityDialog = ref<"compress" | "extract" | "downloadUrl" | null>(null);
const fileUtilityTargetPaneId = ref<string | null>(null);
const fileUtilityName = ref("");
const fileUtilityUrl = ref("");
const cleanupRetainTarget = ref<SftpTransferSummary | SftpTransferIntentSummary | null>(null);
const cleanupPendingTransferId = ref<string | null>(null);
const retainedForExit = ref(new Set<string>());
const closeRemotePaneTargetId = ref<string | null>(null);
const overwritePrompt = ref<OverwritePromptState | null>(null);
const permissionsTarget = ref<PermissionsTarget | null>(null);
const permissionMode = ref(0);
const permissionsPending = ref(false);
const activeSftpPaneId = ref(remotePaneId);
const dragIntent = ref<SftpPaneDragIntent | null>(null);
const dropPaneId = ref<string | null>(null);
const pointerDrag = ref<PointerDragState | null>(null);
const contextMenu = ref<SftpContextMenuState | null>(null);
const contextMenuRoot = ref<HTMLElement | null>(null);
const previewOpen = ref(false);
const previewLoading = ref(false);
const previewName = ref("");
const previewContent = ref<SftpFilePreviewContent | null>(null);
const previewError = ref(false);
const previewImageUrl = ref<string | null>(null);
const previewText = ref("");
const previewOriginalText = ref("");
const previewTextEditable = ref(false);
const previewTextLineEnding = ref<SftpTextLineEnding>("lf");
const previewTailActive = ref(false);
const previewTailUsed = ref(false);
const previewTailOffset = ref("0");
const previewTailError = ref(false);
const previewTailReset = ref(false);
const previewTailTrimmed = ref(false);
const previewSaving = ref(false);
const previewSaveError = ref(false);
const previewTarget = ref<TextPreviewTarget | null>(null);
const previewMode = ref<"preview" | "tail">("preview");
const nextCursorByPane = reactive<Record<string, number[] | null>>({});
const pathDraftByPane = reactive<Record<string, string>>({});
const pathSuggestionsOpenByPane = reactive<Record<string, boolean>>({});
const pathSuggestionIndexByPane = reactive<Record<string, number>>({});
const searchOpenByPane = reactive<Record<string, boolean>>({});
const pendingPaneIds = reactive(new Set<string>());
const localTrailByPane = new Map<string, LocalTrailItem[]>();
const localFilesystemPathByPane = new Map<string, string>();
let sftpViewMounted = false;
let refreshTimer: number | null = null;
let stopNativeFileDrop: (() => void) | null = null;
let previewRequestRevision = 0;
let previewTailTimer: number | null = null;
let previewTailDecoder = new TextDecoder();
let previewTailPendingCarriageReturn = false;
let appliedSessionSnapshotRevision: string | null = null;
let appliedIntentSnapshotRevision: string | null = null;
const transferTargetRefreshes = new Map<string, TransferTargetRefresh>();
const completedTransferRefreshes = new Set<string>();

const paneStates = reactive<Record<string, SftpPaneState>>({
  [localPaneId]: createSftpPaneState(localPaneId, "local", browserPreferences.browser),
  [remotePaneId]: createSftpPaneState(remotePaneId, "remote", browserPreferences.browser),
});
const sftpLayout = ref<TerminalLayoutNode>({
  kind: "split", splitId: "sftp-initial-split", direction: "horizontal", ratio: 0.5,
  first: createTerminalPane(localPaneId, "local"), second: createTerminalPane(remotePaneId, "remote"),
});

const activePane = computed(() => paneStates[activeSftpPaneId.value] ?? null);
const closeRemotePanePending = computed(() => {
  const paneId = closeRemotePaneTargetId.value;
  return paneId ? paneInteractionPending(paneId) : false;
});
const mutationPane = computed(() => {
  const paneId = mutationTargetPaneId.value;
  return paneId ? paneStates[paneId] ?? null : null;
});
const mutationPanePending = computed(() => Boolean(mutationPane.value && paneInteractionPending(mutationPane.value.paneId)));
const fileUtilityPane = computed(() => {
  const paneId = fileUtilityTargetPaneId.value;
  return paneId ? paneStates[paneId] ?? null : null;
});
const fileUtilityPanePending = computed(() => Boolean(fileUtilityPane.value && paneInteractionPending(fileUtilityPane.value.paneId)));
const hostOptions = computed(() => hosts.value.map((host) => ({ value: host.hostId, label: `${host.label} · ${host.normalizedAddress}:${host.port}` })));
const sortOptions = computed(() => ([
  { value: "name", label: t("sftp.sorts.name") },
  { value: "size", label: t("sftp.sorts.size") },
  { value: "modified", label: t("sftp.sorts.modified") },
]));
const intentTransferIds = computed(() => new Set(intentTransfers.value.map((transfer) => transfer.transferId)));
const displayedLegacyTransfers = computed(() => legacyTransfers.value.filter(
  (transfer) => !intentTransferIds.value.has(transfer.transferId),
));
const allTransferCount = computed(() => displayedLegacyTransfers.value.length + intentTransfers.value.length);
const activeTransfers = computed(() => [...intentTransfers.value, ...displayedLegacyTransfers.value].filter(
  (transfer) => ["queued", "preparing", "transferring", "verifying", "committing", "cancelling"].includes(transfer.state),
));
const activeTransferProgress = computed(() => {
  if (!activeTransfers.value.length) return null;
  const total = activeTransfers.value.reduce((sum, transfer) => sum + transfer.expectedBytes, 0);
  if (total <= 0) return null;
  const done = activeTransfers.value.reduce((sum, transfer) => sum + Math.min(transfer.transferredBytes, transfer.expectedBytes), 0);
  return Math.round(Math.min(100, Math.max(0, done / total * 100)));
});
const modifiedDateFormatter = computed(() => new Intl.DateTimeFormat(locale.value, { dateStyle: "medium" }));
const transferNumberFormatter = computed(() => new Intl.NumberFormat(locale.value, { maximumFractionDigits: 1 }));
const permissionGroups = [
  { key: "owner", bits: [0o400, 0o200, 0o100] },
  { key: "group", bits: [0o040, 0o020, 0o010] },
  { key: "others", bits: [0o004, 0o002, 0o001] },
] as const;
const permissionActions = ["read", "write", "execute"] as const;
const permissionModeLabel = computed(() => permissionMode.value.toString(8).padStart(3, "0"));
const activeSelectedEntries = computed(() => activePane.value
  ? activePane.value.entries.filter((entry) => activePane.value?.selectedEntryKeys.includes(entry.key))
  : []);
const contextSelectedEntry = computed(() => {
  const pane = contextMenu.value ? paneStates[contextMenu.value.paneId] : null;
  return pane?.entries.find((entry) => entry.key === pane.selectedEntryKey) ?? null;
});
const contextSelectedEntries = computed(() => {
  const pane = contextMenu.value ? paneStates[contextMenu.value.paneId] : null;
  return pane?.entries.filter((entry) => pane.selectedEntryKeys.includes(entry.key)) ?? [];
});
const previewDirty = computed(() => previewText.value !== previewOriginalText.value);
const previewSaveTooLarge = computed(() => encoder.encode(
  previewText.value.replace(/\n/g, textLineEnding(previewTextLineEnding.value)),
).byteLength > 1024 * 1024);
const previewEditorReadonly = computed(() => previewMode.value === "tail" || !previewTextEditable.value);
const mutationNameValid = computed(() => {
  const value = mutationName.value.trim();
  return value.length > 0 && value !== "." && value !== ".." && !value.includes("/") && !value.includes("\0");
});
const fileUtilityNameValid = computed(() => {
  const value = fileUtilityName.value.trim();
  return value.length > 0 && value !== "." && value !== ".." && !value.includes("/") && !value.includes("\\") && !value.includes("\0");
});
const fileUtilityValid = computed(() => {
  if (!fileUtilityDialog.value || !fileUtilityNameValid.value) return false;
  if (fileUtilityDialog.value !== "downloadUrl") return true;
  try {
    const url = new URL(fileUtilityUrl.value.trim());
    return url.protocol === "https:" && !url.username && !url.password;
  } catch { return false }
});

function showOperationFailed() {
  tips.show({
    scope: operationFeedbackScope,
    tone: "error",
    title: t("sftp.operationFailed"),
  });
}
function showFileUtilityFailed(error: unknown) {
  const code = parseCoreApiError(error)?.code;
  const reason = code === "sftp.conflict" ? "conflict"
    : code === "sftp.invalid_input" ? "invalidInput"
      : code === "sftp.unsafe_no_replace_unsupported" ? "unsafeCommit"
        : code === "sftp.cleanup_incomplete" ? "cleanupIncomplete"
          : code === "sftp.length_mismatch" ? "lengthMismatch" : "protocol";
  tips.show({
    scope: operationFeedbackScope,
    tone: "error",
    title: t(`sftp.fileUtilities.${fileUtilityDialog.value ?? "compress"}.title`),
    message: t(`sftp.fileUtilities.failures.${reason}`),
  });
}
function showDirectoryMemorySaveFailed() {
  tips.show({
    scope: "sftp-directory-memory",
    tone: "error",
    title: t("sftpSettings.directoryMemorySaveFailed"),
  });
}
function showDirectoryMemoryRestoreFailed() {
  tips.show({
    scope: "sftp-directory-memory",
    tone: "error",
    title: t("sftpSettings.directoryMemoryRestoreFailed"),
  });
}
function rememberSuccessfulLocalDirectory(pane: SftpPaneState) {
  const path = localFilesystemPathByPane.get(pane.paneId);
  if (browserPreferences.rememberLastDirectory && path && !browserPreferences.rememberLocalDirectory(path)) showDirectoryMemorySaveFailed();
}
function rememberSuccessfulRemoteDirectory(pane: SftpPaneState, hostId: string, pathBytes: number[]) {
  if (browserPreferences.rememberLastDirectory
    && (pane.endpoint.kind !== "remote" || pane.endpoint.hostId !== hostId || !browserPreferences.rememberRemoteDirectory(hostId, pathBytes))) {
    showDirectoryMemorySaveFailed();
  }
}
function clearInvisiblePaneSelection(pane: SftpPaneState) {
  const visible = new Set(visibleSftpPaneEntries(pane).map((entry) => entry.key));
  const selected = pane.selectedEntryKeys.filter((key) => visible.has(key));
  if (selected.length === pane.selectedEntryKeys.length) return;
  pane.selectedEntryKeys = selected;
  pane.selectedEntryKey = selected.includes(pane.selectedEntryKey ?? "")
    ? pane.selectedEntryKey
    : selected.at(-1) ?? null;
  pane.selectionAnchorKey = selected.includes(pane.selectionAnchorKey ?? "")
    ? pane.selectionAnchorKey
    : pane.selectedEntryKey;
  pane.selectionRevision += 1;
  if (dragIntent.value?.sourcePaneId === pane.paneId) dragIntent.value = null;
  if (pointerDrag.value?.sourcePaneId === pane.paneId) pointerDrag.value = null;
  if (dropPaneId.value === pane.paneId) dropPaneId.value = null;
}
function togglePaneShowHidden(pane: SftpPaneState) {
  pane.showHidden = !pane.showHidden;
  clearInvisiblePaneSelection(pane);
}
function togglePaneFoldersFirst(pane: SftpPaneState) {
  pane.foldersFirst = !pane.foldersFirst;
  clearInvisiblePaneSelection(pane);
}
function setPaneSearch(pane: SftpPaneState, value: string) {
  pane.search = value;
  clearInvisiblePaneSelection(pane);
}

function sftpPaneKind(pane: TerminalPaneNode) { return pane.terminalId === "local" ? "local" : "remote" }
function paneStillActive(pane: SftpPaneState, endpointRevision: number) {
  return sftpViewMounted && paneStates[pane.paneId] === pane && pane.endpointRevision === endpointRevision;
}
async function releaseLocalCapability(capability: SftpLocalDirectoryCapability) {
  await releaseSftpLocalDirectory({
    directoryRef: capability.directoryRef,
    expectedRevision: capability.revision,
  }).catch(() => undefined);
}
function setRememberableLocalPath(pane: SftpPaneState, capability: SftpLocalDirectoryCapability) {
  if (capability.rememberablePath) localFilesystemPathByPane.set(pane.paneId, capability.rememberablePath);
  else localFilesystemPathByPane.delete(pane.paneId);
}
function sessionForPane(pane: SftpPaneState) {
  const endpoint = pane.endpoint;
  if (endpoint.kind !== "remote" || !endpoint.sessionId) return null;
  return sessions.value.find((item) => item.sessionId === endpoint.sessionId) ?? null;
}
function unclaimedSftpSessionForHost(hostId: string, includeFailed = false) {
  const claimedSessionIds = new Set(
    Object.values(paneStates)
      .filter((pane) => pane.endpoint.kind === "remote" && pane.endpoint.sessionId)
      .map((pane) => pane.endpoint.kind === "remote" ? pane.endpoint.sessionId : null),
  );
  const available = sessions.value.filter((session) => session.hostId === hostId
    && !claimedSessionIds.has(session.sessionId));
  return available.find((session) => session.state === "ready")
    ?? (includeFailed ? available.find((session) => session.state === "failed") : null)
    ?? null;
}
function attachSftpSessionToPane(pane: SftpPaneState, session: SftpSessionSummary) {
  replaceSftpPaneEndpoint(pane, {
    kind: "remote",
    hostId: session.hostId,
    sessionId: session.sessionId,
    generation: session.generation,
  }, "/", [47]);
}
function paneHost(pane: SftpPaneState) {
  const endpoint = pane.endpoint;
  return endpoint.kind === "remote" ? hosts.value.find((host) => host.hostId === endpoint.hostId) ?? null : null;
}
function paneRemoteLabel(pane: SftpPaneState) {
  const parent = sessionForPane(pane)?.parentSshSession;
  return paneHost(pane)?.label ?? (parent ? parentSessionLabels[parent.sessionId] : null) ?? t("sftp.parentConnection");
}

async function consumePluginNavigation() {
  if (!pageObserversActive || !navigationReady || navigationWorking || loading.value || operationPending.value
    || previewOpen.value || previewSaving.value || mutationDialog.value || fileUtilityDialog.value || overwritePrompt.value
    || !pendingSftpPluginNavigations.value.length) return;
  navigationWorking = true;
  const intent = takeSftpPluginNavigation();
  if (!intent) { navigationWorking = false; return; }
  operationPending.value = true;
  let pane: SftpPaneState | undefined;
  let fence: ReturnType<typeof beginSftpPaneDirectoryLoad> | undefined;
  let panePending = false;
  try {
    await refreshSnapshot();
    if (!navigationReady) return;
    const requested = intent.sftpSession;
    const session = sessions.value.find((item) => item.sessionId === requested.sessionId
      && item.generation === requested.generation && item.state === "ready"
      && item.parentSshSession?.sessionId === requested.parentSshSession?.sessionId
      && item.parentSshSession?.generation === requested.parentSshSession?.generation);
    if (!session?.parentSshSession) throw new Error("staleSftpChild");
    const parent = session.parentSshSession;
    const ssh = (await fetchSshSessionSnapshot()).sessions.find((item) => item.sessionId === parent.sessionId
      && item.generation === parent.generation && item.state === "running");
    if (!ssh || !navigationReady) throw new Error("staleSshParent");
    const sshTarget = ssh.target;
    parentSessionLabels[parent.sessionId] = sshTarget.kind === "host"
      ? hosts.value.find((item) => item.hostId === sshTarget.hostId)?.label ?? t("sftp.parentConnection")
      : `${sshTarget.endpoint.username ? `${sshTarget.endpoint.username}@` : ""}${sshTarget.endpoint.address}:${sshTarget.endpoint.port}`;
    pane = Object.values(paneStates).find((item) => item.endpoint.kind === "remote"
      && item.endpoint.sessionId === session.sessionId && !paneInteractionPending(item.paneId));
    pane ??= Object.values(paneStates).find((item) => item.endpoint.kind === "remote"
      && !item.endpoint.sessionId && !paneInteractionPending(item.paneId));
    if (!pane) {
      const sourceId = activeSftpPaneId.value;
      const paneId = crypto.randomUUID();
      sftpLayout.value = splitTerminalPane(sftpLayout.value, sourceId, "horizontal", paneId, crypto.randomUUID());
      sftpLayout.value = setTerminalForPane(sftpLayout.value, paneId, "remote");
      paneStates[paneId] = createSftpPaneState(paneId, "remote", browserPreferences.browser);
      pane = paneStates[paneId]!;
    }
    const targetPane = pane;
    pendingPaneIds.add(targetPane.paneId);
    panePending = true;
    await cancelRemotePaneCursor(targetPane);
    attachSftpSessionToPane(targetPane, session);
    targetPane.search = "";
    activeSftpPaneId.value = targetPane.paneId;
    fence = beginSftpPaneDirectoryLoad(targetPane);
    const endpointRevision = targetPane.endpointRevision;
    const current = () => navigationReady && paneStates[targetPane.paneId] === targetPane
      && targetPane.endpointRevision === endpointRevision
      && sessionForPane(targetPane)?.generation === requested.generation
      && sessionForPane(targetPane)?.state === "ready";
    const result = await resolveSftpPluginNavigation(intent, { list: listSftpDirectory, cancel: cancelSftpDirectoryListing }, current);
    if (!current() || !completeSftpPaneDirectoryLoad(targetPane, fence,
      decoder.decode(new Uint8Array(result.listing.path.bytes)), result.listing.entries.map(mapRemoteEntry),
      result.listing.path.bytes, result.listing.directoryRef)) {
      if (result.listing.nextCursor) await cancelSftpDirectoryListing({ sessionId: result.listing.sessionId, expectedGeneration: result.listing.generation, path: result.listing.path, cursor: result.listing.nextCursor }).catch(() => undefined);
      return;
    }
    nextCursorByPane[targetPane.paneId] = result.listing.nextCursor;
    pathDraftByPane[targetPane.paneId] = targetPane.directory;
    if (result.entry) selectSftpPaneEntry(targetPane, result.entry.entryRef);
    if (intent.request.edit) {
      const entry = targetPane.entries.find((item) => item.key === targetPane.selectedEntryKey) ?? null;
      if (previewKindForEntry(entry) !== "text") targetPane.error = t("sftp.previewUnavailable");
      else await previewSelectedFile(targetPane, "preview", true);
    } else if (result.entry) {
      await nextTick();
      document.querySelector<HTMLElement>(`[data-pane-id="${CSS.escape(targetPane.paneId)}"] [aria-selected="true"]`)?.scrollIntoView?.({ block: "nearest" });
    }
  } catch {
    if (navigationReady) {
      const message = t("sftp.navigationUnavailable");
      tips.show({ scope: navigationFeedbackScope, tone: "error", title: message });
      if (pane && fence) failSftpPaneDirectoryLoad(pane, fence, message);
    }
  } finally {
    if (pane && panePending) pendingPaneIds.delete(pane.paneId);
    operationPending.value = false;
    navigationWorking = false;
  }
}
watch([pendingSftpPluginNavigations, loading, operationPending, previewOpen, previewSaving, mutationDialog, fileUtilityDialog, overwritePrompt], () => { void consumePluginNavigation(); });
function paneReady(pane: SftpPaneState) {
  if (pane.endpoint.kind === "local") return Boolean(pane.directoryRef && pane.endpoint.revision);
  const session = sessionForPane(pane);
  return session?.state === "ready"
    && session.generation === pane.endpoint.generation
    && Boolean(pane.directoryRef);
}
function paneEntries(paneId: string) {
  const pane = paneStates[paneId];
  return pane && paneReady(pane) ? visibleSftpPaneEntries(pane) : [];
}
function paneState(paneId: string) { return paneStates[paneId]! }
function paneInteractionPending(paneId: string) { return pendingPaneIds.has(paneId) }
function paneHostId(pane: SftpPaneState) {
  return pane.endpoint.kind === "remote" ? pane.endpoint.hostId ?? "" : "";
}
function paneHasSession(pane: SftpPaneState) {
  return pane.endpoint.kind === "remote" && Boolean(pane.endpoint.sessionId);
}
function setPaneHostId(pane: SftpPaneState, hostId: string) {
  if (pane.endpoint.kind !== "remote" || pane.endpoint.sessionId) return;
  replaceSftpPaneEndpoint(pane, { kind: "remote", hostId: hostId || null, sessionId: null, generation: null }, "/", [47]);
}
function pathSuggestions(pane: SftpPaneState) {
  const query = (pathDraftByPane[pane.paneId] ?? pane.directory).trim().toLocaleLowerCase(locale.value);
  const candidates = [
    pane.directory,
    ...pane.entries
      .flatMap((entry) => pane.endpoint.kind === "remote" && entry.kind === "directory" && entry.remotePathBytes
        ? [decoder.decode(new Uint8Array(entry.remotePathBytes))]
        : []),
  ];
  return [...new Set(candidates)]
    .filter((candidate) => candidate.toLocaleLowerCase(locale.value).includes(query))
    .slice(0, 8);
}
function updatePathDraft(pane: SftpPaneState, value: string) {
  pathDraftByPane[pane.paneId] = value;
  pathSuggestionsOpenByPane[pane.paneId] = true;
  pathSuggestionIndexByPane[pane.paneId] = -1;
}
function choosePathSuggestion(pane: SftpPaneState, value: string) {
  pathDraftByPane[pane.paneId] = value;
  pathSuggestionsOpenByPane[pane.paneId] = false;
  pathSuggestionIndexByPane[pane.paneId] = -1;
  void openTypedPath(pane);
}
function onPathKeydown(event: KeyboardEvent, pane: SftpPaneState) {
  const suggestions = pathSuggestions(pane);
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    if (!suggestions.length) return;
    event.preventDefault();
    pathSuggestionsOpenByPane[pane.paneId] = true;
    const current = pathSuggestionIndexByPane[pane.paneId] ?? -1;
    pathSuggestionIndexByPane[pane.paneId] = event.key === "ArrowDown"
      ? (current + 1 + suggestions.length) % suggestions.length
      : (current - 1 + suggestions.length) % suggestions.length;
    return;
  }
  if (event.key === "Enter") {
    event.preventDefault();
    const index = pathSuggestionIndexByPane[pane.paneId] ?? -1;
    if (pathSuggestionsOpenByPane[pane.paneId] && suggestions[index]) {
      choosePathSuggestion(pane, suggestions[index]!);
    } else {
      pathSuggestionsOpenByPane[pane.paneId] = false;
      void openTypedPath(pane);
    }
    return;
  }
  if (event.key === "Escape") {
    event.preventDefault();
    pathSuggestionsOpenByPane[pane.paneId] = false;
    pathSuggestionIndexByPane[pane.paneId] = -1;
    pathDraftByPane[pane.paneId] = pane.directory;
  }
}
function requestOverwriteConfirmation(displayName: string, kind: "file" | "directory", allowReplaceAll = false) {
  if (overwritePrompt.value) return Promise.resolve<OverwriteDecision>("skip");
  return new Promise<OverwriteDecision>((resolve) => {
    overwritePrompt.value = { displayName, kind, allowReplaceAll, resolve };
  });
}
function settleOverwriteConfirmation(decision: OverwriteDecision) {
  const prompt = overwritePrompt.value;
  overwritePrompt.value = null;
  prompt?.resolve(decision);
}
function matchingTargetEntry(target: SftpPaneState, displayName: string) {
  return target.entries.find((entry) => entry.displayName === displayName) ?? null;
}
function stateTone(state: SftpSessionState | SftpTransferState) {
  if (state === "ready" || state === "completed") return "success" as const;
  if (state === "failed") return "danger" as const;
  if (["connecting", "authenticating", "openingSubsystem", "transferring", "verifying", "committing"].includes(state)) return "info" as const;
  return "neutral" as const;
}
function sessionFailureLabel(code: SftpFailureCode) {
  return t(`sftp.sessionFailures.${code}`);
}
// Snapshot polling must not replay an unchanged failure or steal Pane focus.
const notifiedSessionFailures = new Map<string, string>();
watch(() => Object.values(paneStates).map(sessionForPane), (paneSessions) => {
  const failures = new Map(paneSessions.filter((session) => session?.failure)
    .map((session) => [session!.sessionId, session!]));
  for (const sessionId of notifiedSessionFailures.keys()) {
    if (!failures.has(sessionId)) {
      notifiedSessionFailures.delete(sessionId);
      tips.dismissScope(`sftp-session:${sessionId}`);
    }
  }
  for (const [sessionId, session] of failures) {
    const failure = session.failure!;
    const signature = `${session.generation}:${failure.code}`;
    if (notifiedSessionFailures.get(sessionId) === signature) continue;
    notifiedSessionFailures.set(sessionId, signature);
    tips.show({
      scope: `sftp-session:${sessionId}`,
      tone: "error",
      title: sessionFailureLabel(failure.code),
      message: hosts.value.find((host) => host.hostId === session.hostId)?.label,
    });
  }
});

function paneSessionFailure(pane: SftpPaneState) {
  return sessionForPane(pane)?.failure;
}

function sessionFailureNeedsTerminal(code: SftpFailureCode) {
  return [
    "hostKeyMismatch",
    "credentialUnavailable",
    "authenticationRejected",
  ].includes(code);
}
function paneConnectionLabel(pane: SftpPaneState) {
  const failure = paneSessionFailure(pane);
  if (failure?.code === "vaultLocked") return t("sftp.unlockVaultAndRetry");
  if (failure && sessionFailureNeedsTerminal(failure.code)) return t("sftp.openTerminalToResolve");
  return t("sftp.connect");
}
function recoverRemotePane(pane: SftpPaneState) {
  const failure = paneSessionFailure(pane);
  return failure && sessionFailureNeedsTerminal(failure.code)
    ? openHostInTerminal(pane)
    : connectRemotePane(pane);
}
async function openHostInTerminal(pane: SftpPaneState) {
  const host = pane ? paneHost(pane) : null;
  if (!host) return;
  await router.push({
    path: "/terminal",
    query: {
      hostId: host.hostId,
      source: "sftp",
      connectOperationId: crypto.randomUUID(),
    },
  });
}
function remotePath(bytes: number[]) { return { bytes } }
function joinRemoteBytes(parentBytes: number[], name: string) {
  const parent = decoder.decode(new Uint8Array(parentBytes));
  return Array.from(encoder.encode(`${parent === "/" ? "" : parent}/${name}`));
}
function parentRemoteBytes(bytes: number[]) {
  const path = decoder.decode(new Uint8Array(bytes));
  if (path === "/") return [47];
  return Array.from(encoder.encode(path.slice(0, path.lastIndexOf("/")) || "/"));
}
function formatModified(value: number | null) { return value === null ? "—" : modifiedDateFormatter.value.format(new Date(value)) }
function olderWireSequence(candidate: string, applied: string | null) {
  return applied !== null && BigInt(candidate) < BigInt(applied);
}
function progress(transfer: { expectedBytes: number; transferredBytes: number }) {
  return transfer.expectedBytes > 0 ? (transfer.transferredBytes / transfer.expectedBytes) * 100 : 0;
}
function formatTransferBytes(value: number) {
  if (!Number.isFinite(value) || value < 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${transferNumberFormatter.value.format(unit === 0 ? Math.round(size) : size)} ${units[unit]}`;
}
function toggleTransferActivity() {
  transfersOpen.value = !transfersOpen.value;
}
function closeTransferActivity(restoreFocus = false) {
  if (!transfersOpen.value) return;
  transfersOpen.value = false;
  if (restoreFocus) {
    transferActivityRoot.value?.querySelector<HTMLButtonElement>("[data-sftp-transfer-toggle]")?.focus();
  }
}
function onDocumentPointerDown(event: PointerEvent) {
  if (transfersOpen.value && !transferActivityRoot.value?.contains(event.target as Node)) closeTransferActivity();
  if (contextMenu.value && !contextMenuRoot.value?.contains(event.target as Node)) contextMenu.value = null;
}
function onDocumentKeyDown(event: KeyboardEvent) {
  if (event.key === "Escape" && transfersOpen.value) {
    event.preventDefault();
    closeTransferActivity(true);
  }
  if (event.key === "Escape" && contextMenu.value) {
    event.preventDefault();
    contextMenu.value = null;
  }
}
function splitSftpPane(paneId: string, direction: TerminalSplitDirection, targetKind?: "local" | "remote") {
  const sourceNode = findTerminalPane(sftpLayout.value, paneId);
  const sourceState = paneStates[paneId];
  if (!sourceNode || !sourceState || paneInteractionPending(paneId)) return;
  const kind = targetKind ?? sftpPaneKind(sourceNode);
  const newPaneId = crypto.randomUUID();
  sftpLayout.value = splitTerminalPane(sftpLayout.value, paneId, direction, newPaneId, crypto.randomUUID());
  sftpLayout.value = setTerminalForPane(sftpLayout.value, newPaneId, kind);
  paneStates[newPaneId] = kind === sftpPaneKind(sourceNode)
    ? cloneSftpPaneState(sourceState, newPaneId)
    : createSftpPaneState(newPaneId, kind, browserPreferences.browser);
  if (kind === "remote" && kind !== sftpPaneKind(sourceNode) && hosts.value[0]) {
    replaceSftpPaneEndpoint(paneStates[newPaneId]!, {
      kind: "remote", hostId: hosts.value[0].hostId, sessionId: null, generation: null,
    }, "/", [47]);
  }
  nextCursorByPane[newPaneId] = null;
  activeSftpPaneId.value = newPaneId;
  if (paneStates[newPaneId]?.endpoint.kind === "local") {
    void initializeDefaultLocalPaneSafely(paneStates[newPaneId]!);
  }
}
function resizeSftpSplit(splitId: string, ratio: number) { sftpLayout.value = setTerminalSplitRatio(sftpLayout.value, splitId, ratio) }

async function releaseLocalPane(paneId: string) {
  const pane = paneStates[paneId];
  const capabilities = [...(localTrailByPane.get(paneId) ?? [])];
  if (pane?.endpoint.kind === "local" && pane.endpoint.directoryRef && pane.endpoint.revision) {
    capabilities.push({
      capability: {
        directoryRef: pane.endpoint.directoryRef,
        revision: pane.endpoint.revision,
        displayName: pane.endpoint.displayPath,
        rememberablePath: null,
      },
      displayPath: pane.endpoint.displayPath,
      rememberedPath: null,
    });
  }
  localTrailByPane.delete(paneId);
  localFilesystemPathByPane.delete(paneId);
  await Promise.allSettled(capabilities.map(({ capability }) => releaseSftpLocalDirectory({ directoryRef: capability.directoryRef, expectedRevision: capability.revision })));
}
async function cancelRemotePaneCursor(pane: SftpPaneState | undefined) {
  if (!pane) return;
  const cursor = nextCursorByPane[pane.paneId];
  if (!cursor || pane.endpoint.kind !== "remote" || !pane.endpoint.sessionId
    || !pane.endpoint.generation || !pane.remoteDirectoryPathBytes) return;
  nextCursorByPane[pane.paneId] = null;
  await cancelSftpDirectoryListing({
    sessionId: pane.endpoint.sessionId,
    expectedGeneration: pane.endpoint.generation,
    path: remotePath(pane.remoteDirectoryPathBytes),
    cursor,
  }).catch(() => undefined);
}
async function finalizeCloseSftpPane(paneId: string, allowPanePending = false) {
  if (countTerminalPanes(sftpLayout.value) <= 1 || (!allowPanePending && paneInteractionPending(paneId))) return;
  const closed = closeTerminalPane(sftpLayout.value, paneId);
  sftpLayout.value = closed.node;
  await cancelRemotePaneCursor(paneStates[paneId]);
  await releaseLocalPane(paneId);
  delete paneStates[paneId];
  delete nextCursorByPane[paneId];
  delete pathDraftByPane[paneId];
  delete pathSuggestionsOpenByPane[paneId];
  delete pathSuggestionIndexByPane[paneId];
  delete searchOpenByPane[paneId];
  activeSftpPaneId.value = closed.nextActivePaneId;
}

async function requestCloseSftpPane(paneId: string) {
  const pane = paneStates[paneId];
  if (!pane || countTerminalPanes(sftpLayout.value) <= 1 || paneInteractionPending(paneId)) return;
  const session = sessionForPane(pane);
  if (pane.endpoint.kind === "remote" && session && session.state !== "closed") {
    closeRemotePaneTargetId.value = paneId;
    return;
  }
  await finalizeCloseSftpPane(paneId);
}

async function disconnectAndCloseRemotePane() {
  const paneId = closeRemotePaneTargetId.value;
  const pane = paneId ? paneStates[paneId] : null;
  const session = pane ? sessionForPane(pane) : null;
  if (!paneId || !pane || pane.endpoint.kind !== "remote" || !session
    || session.state === "closed" || pane.endpoint.generation !== session.generation
    || paneInteractionPending(paneId)) return;
  pendingPaneIds.add(paneId);
  try {
    await disconnectSftpSession({
      sessionId: session.sessionId,
      expectedGeneration: session.generation,
    });
    closeRemotePaneTargetId.value = null;
    await finalizeCloseSftpPane(paneId, true);
    await refreshSnapshot();
  } catch {
    pane.error = t("sftp.closeRemotePaneFailed");
    showOperationFailed();
  } finally {
    pendingPaneIds.delete(paneId);
  }
}

async function refreshSnapshot() {
  if (!canUseDesktopCore()) return;
  const [sessionSnapshot, intentSnapshot] = await Promise.all([fetchSftpSessionSnapshot(), fetchSftpTransferIntentSnapshot()]);
  const completedTransferIds = [
    ...sessionSnapshot.transfers.filter((transfer) => transfer.state === "completed").map((transfer) => transfer.transferId),
    ...intentSnapshot.transfers.filter((transfer) => transfer.state === "completed").map((transfer) => transfer.transferId),
  ];
  if (!olderWireSequence(sessionSnapshot.snapshotRevision, appliedSessionSnapshotRevision)) {
    appliedSessionSnapshotRevision = sessionSnapshot.snapshotRevision;
    sessions.value = sessionSnapshot.sessions;
    legacyTransfers.value = sessionSnapshot.transfers;
    for (const pane of Object.values(paneStates)) {
      if (pane.endpoint.kind !== "remote" || !pane.endpoint.sessionId) continue;
      const paneSessionId = pane.endpoint.sessionId;
      const session = sessionSnapshot.sessions.find((item) => item.sessionId === paneSessionId);
      if (session && session.generation !== pane.endpoint.generation) {
        await cancelRemotePaneCursor(pane);
      }
      if (session && reconcileSftpPaneRemoteGeneration(
        pane,
        session.hostId,
        session.sessionId,
        session.generation,
      )) {
        pane.error = t("sftp.paneGenerationChanged");
      }
    }
  }
  if (!olderWireSequence(intentSnapshot.snapshotRevision, appliedIntentSnapshotRevision)) {
    appliedIntentSnapshotRevision = intentSnapshot.snapshotRevision;
    intentTransfers.value = intentSnapshot.transfers;
  }
  const newlyReadyPanes = Object.values(paneStates).filter((pane) => pane.endpoint.kind === "remote"
    && pane.endpoint.sessionId
    && !pane.directoryRef
    && !pane.loading
    && !pane.error
    && !paneInteractionPending(pane.paneId)
    && sessionForPane(pane)?.state === "ready"
    && sessionForPane(pane)?.generation === pane.endpoint.generation);
  await Promise.all(newlyReadyPanes.map((pane) => pane.endpoint.kind === "remote" && pane.endpoint.hostId
    ? loadInitialRemoteDirectory(pane)
    : loadRemoteDirectory(pane, pane.remoteDirectoryPathBytes ?? [47])));
  await refreshCompletedTransferTargets(completedTransferIds);
}

function rememberTransferTarget(transferId: string | undefined, pane: SftpPaneState) {
  if (!transferId) return;
  transferTargetRefreshes.set(transferId, {
    paneId: pane.paneId,
    endpointRevision: pane.endpointRevision,
    directoryRevision: pane.directoryRevision,
  });
}

async function refreshCompletedTransferTargets(transferIds: string[]) {
  const panes = new Map<string, SftpPaneState>();
  for (const transferId of transferIds) {
    if (completedTransferRefreshes.has(transferId)) continue;
    completedTransferRefreshes.add(transferId);
    const target = transferTargetRefreshes.get(transferId);
    transferTargetRefreshes.delete(transferId);
    if (!target) continue;
    const pane = paneStates[target.paneId];
    if (!pane || pane.endpointRevision !== target.endpointRevision
      || pane.directoryRevision !== target.directoryRevision || paneInteractionPending(pane.paneId)) continue;
    panes.set(pane.paneId, pane);
  }
  await Promise.all([...panes.values()].map((pane) => pane.endpoint.kind === "local" ? loadLocalDirectory(pane) : loadRemoteDirectory(pane)));
}
async function connectRemotePane(pane: SftpPaneState) {
  const host = paneHost(pane);
  if (!pane || pane.endpoint.kind !== "remote" || !host || paneInteractionPending(pane.paneId)) return;
  activeSftpPaneId.value = pane.paneId;
  pendingPaneIds.add(pane.paneId);
  try {
    const reusableSession = pane.endpoint.sessionId ? null : unclaimedSftpSessionForHost(host.hostId);
    const endpointRevision = pane.endpointRevision;
    const sessionId = pane.endpoint.sessionId;
    const generation = pane.endpoint.generation;
    const stillCurrent = () => sftpViewMounted && paneStates[pane.paneId] === pane
      && pane.endpointRevision === endpointRevision && pane.endpoint.kind === "remote"
      && pane.endpoint.hostId === host.hostId && pane.endpoint.sessionId === sessionId
      && pane.endpoint.generation === generation;
    if (!reusableSession) {
      const recoverVault = sessions.value.find((session) => session.sessionId === sessionId)?.failure?.code === "vaultLocked";
      if (!await ensureHostVault(host.hostId, host.stateVersion, recoverVault) || !stillCurrent()) return;
    }
    const summary = reusableSession ?? await openSftpSession({ hostId: host.hostId, expectedHostStateVersion: host.stateVersion, ...(sessionId ? { sessionId } : {}) });
    if (!stillCurrent()) return;
    attachSftpSessionToPane(pane, summary);
    await refreshSnapshot();
    if (summary.state === "ready") await loadInitialRemoteDirectory(pane, true);
  } catch {
    showOperationFailed();
    await refreshSnapshot().catch(() => undefined);
  } finally { pendingPaneIds.delete(pane.paneId) }
}
async function disconnectRemotePane(pane: SftpPaneState) {
  if (!pane || pane.endpoint.kind !== "remote" || !pane.endpoint.sessionId || !pane.endpoint.generation
    || paneInteractionPending(pane.paneId)) return;
  activeSftpPaneId.value = pane.paneId;
  pendingPaneIds.add(pane.paneId);
  try {
    await cancelRemotePaneCursor(pane);
    await disconnectSftpSession({ sessionId: pane.endpoint.sessionId, expectedGeneration: pane.endpoint.generation });
    replaceSftpPaneEndpoint(pane, { kind: "remote", hostId: pane.endpoint.hostId, sessionId: null, generation: null }, "/", [47]);
    await refreshSnapshot();
  } catch { showOperationFailed(); } finally { pendingPaneIds.delete(pane.paneId) }
}

function mapRemoteEntry(entry: { entryRef: string; path: { bytes: number[] }; displayName: string; kind: SftpPaneEntry["kind"]; size: number | null; modifiedAtUnixMs: number | null; permissionBits: number | null }): SftpPaneEntry {
  return { key: entry.entryRef, entryRef: entry.entryRef, displayName: entry.displayName, kind: entry.kind, nameBytes: [], size: entry.size, modifiedAtUnixMs: entry.modifiedAtUnixMs, permissionBits: entry.permissionBits, remotePathBytes: [...entry.path.bytes], localRelativePath: null, precondition: { kind: entry.kind, size: entry.size, modifiedAtUnixMs: entry.modifiedAtUnixMs } };
}
function mapLocalEntry(entry: { entryRef: string; displayName: string; kind: SftpPaneEntry["kind"]; size: number | null; modifiedAtUnixMs: number | null }): SftpPaneEntry {
  return { key: entry.entryRef, entryRef: entry.entryRef, displayName: entry.displayName, kind: entry.kind, nameBytes: [], size: entry.size, modifiedAtUnixMs: entry.modifiedAtUnixMs, remotePathBytes: null, localRelativePath: null, precondition: { kind: entry.kind, size: entry.size, modifiedAtUnixMs: entry.modifiedAtUnixMs } };
}
async function loadRemoteDirectory(
  pane: SftpPaneState,
  pathBytes = pane.remoteDirectoryPathBytes ?? [47],
  allowPanePending = false,
) {
  const session = sessionForPane(pane);
  if (pane.endpoint.kind !== "remote" || !pane.endpoint.sessionId || !pane.endpoint.generation
    || session?.state !== "ready" || session.generation !== pane.endpoint.generation
    || pane.loading || (!allowPanePending && paneInteractionPending(pane.paneId))) return false;
  const sessionId = pane.endpoint.sessionId;
  const generation = pane.endpoint.generation;
  const hostId = pane.endpoint.hostId;
  await cancelRemotePaneCursor(pane);
  const fence = beginSftpPaneDirectoryLoad(pane);
  try {
    const listing = await listSftpDirectory({ sessionId, expectedGeneration: generation, path: remotePath(pathBytes), cursor: null, pageSize: 256 });
    if (!paneStillActive(pane, fence.endpointRevision)) {
      if (listing.nextCursor) {
        await cancelSftpDirectoryListing({ sessionId: listing.sessionId, expectedGeneration: listing.generation, path: listing.path, cursor: listing.nextCursor }).catch(() => undefined);
      }
      return false;
    }
    if (listing.sessionId !== sessionId || listing.generation !== generation) {
      if (listing.nextCursor) {
        await cancelSftpDirectoryListing({ sessionId: listing.sessionId, expectedGeneration: listing.generation, path: listing.path, cursor: listing.nextCursor }).catch(() => undefined);
      }
      failSftpPaneDirectoryLoad(pane, fence, t("sftp.paneGenerationChanged"));
      return false;
    }
    const applied = completeSftpPaneDirectoryLoad(pane, fence, decoder.decode(new Uint8Array(listing.path.bytes)), listing.entries.map(mapRemoteEntry), listing.path.bytes, listing.directoryRef);
    if (applied) {
      nextCursorByPane[pane.paneId] = listing.nextCursor;
      pathDraftByPane[pane.paneId] = pane.directory;
      if (hostId) rememberSuccessfulRemoteDirectory(pane, hostId, listing.path.bytes);
      return true;
    }
    else if (listing.nextCursor) {
      await cancelSftpDirectoryListing({ sessionId: listing.sessionId, expectedGeneration: listing.generation, path: listing.path, cursor: listing.nextCursor }).catch(() => undefined);
    }
    return false;
  } catch {
    if (paneStillActive(pane, fence.endpointRevision)) failSftpPaneDirectoryLoad(pane, fence, t("sftp.paneLoadFailed"));
    return false;
  }
}
async function loadLocalDirectory(pane: SftpPaneState, allowPanePending = false) {
  if (pane.endpoint.kind !== "local" || !pane.endpoint.directoryRef || !pane.endpoint.revision
    || pane.loading || (!allowPanePending && paneInteractionPending(pane.paneId))) return false;
  const directoryRef = pane.endpoint.directoryRef;
  const revision = pane.endpoint.revision;
  const fence = beginSftpPaneDirectoryLoad(pane);
  try {
    const listing = await listSftpLocalDirectory({ directoryRef, expectedRevision: revision, cursor: null, pageSize: 256 });
    if (!paneStillActive(pane, fence.endpointRevision)) return false;
    if (listing.directoryRef !== directoryRef || listing.revision !== revision) {
      failSftpPaneDirectoryLoad(pane, fence, t("sftp.paneFenceChanged"));
      return false;
    }
    const applied = completeSftpPaneDirectoryLoad(pane, fence, pane.directory, listing.entries.map(mapLocalEntry), null, listing.directoryRef);
    if (applied) {
      nextCursorByPane[pane.paneId] = listing.nextCursor;
      pathDraftByPane[pane.paneId] = pane.directory;
      rememberSuccessfulLocalDirectory(pane);
      return true;
    }
    return false;
  } catch {
    if (paneStillActive(pane, fence.endpointRevision)) failSftpPaneDirectoryLoad(pane, fence, t("sftp.paneLoadFailed"));
    return false;
  }
}

async function firstLocalDirectoryPage(capability: SftpLocalDirectoryCapability) {
  const listing = await listSftpLocalDirectory({
    directoryRef: capability.directoryRef,
    expectedRevision: capability.revision,
    cursor: null,
    pageSize: 256,
  });
  if (listing.directoryRef !== capability.directoryRef || listing.revision !== capability.revision) {
    throw new Error("local directory listing changed");
  }
  return listing;
}

function applyFirstLocalDirectoryPage(
  pane: SftpPaneState,
  listing: Awaited<ReturnType<typeof listSftpLocalDirectory>>,
) {
  const fence = beginSftpPaneDirectoryLoad(pane);
  if (!completeSftpPaneDirectoryLoad(
    pane, fence, pane.directory, listing.entries.map(mapLocalEntry), null, listing.directoryRef,
  )) return;
  nextCursorByPane[pane.paneId] = listing.nextCursor;
  pathDraftByPane[pane.paneId] = pane.directory;
  rememberSuccessfulLocalDirectory(pane);
}

function samePathBytes(left: number[], right: number[]) {
  return left.length === right.length && left.every((byte, index) => byte === right[index]);
}

async function loadInitialRemoteDirectory(pane: SftpPaneState, allowPanePending = false) {
  if (pane.endpoint.kind !== "remote" || !pane.endpoint.hostId || !pane.endpoint.sessionId || !pane.endpoint.generation) return false;
  const endpointRevision = pane.endpointRevision;
  const hostId = pane.endpoint.hostId;
  const sessionId = pane.endpoint.sessionId;
  const generation = pane.endpoint.generation;
  const remembered = browserPreferences.rememberedRemoteDirectory(hostId);
  if (remembered && !samePathBytes(remembered, [47])) {
    if (await loadRemoteDirectory(pane, remembered, allowPanePending)) return true;
    if (!paneStillActive(pane, endpointRevision) || pane.endpoint.kind !== "remote" || pane.endpoint.hostId !== hostId
      || pane.endpoint.sessionId !== sessionId || pane.endpoint.generation !== generation) return false;
    showDirectoryMemoryRestoreFailed();
  }
  if (!paneStillActive(pane, endpointRevision)) return false;
  return loadRemoteDirectory(pane, [47], allowPanePending);
}

async function loadMore(pane: SftpPaneState) {
  const cursor = nextCursorByPane[pane.paneId];
  if (!cursor || pane.loading || !pane.directoryRef || paneInteractionPending(pane.paneId)) return;
  nextCursorByPane[pane.paneId] = null;
  const endpointRevision = pane.endpointRevision;
  const directoryRevision = pane.directoryRevision;
  pane.loading = true;
  try {
    if (pane.endpoint.kind === "local" && pane.endpoint.revision) {
      const listing = await listSftpLocalDirectory({ directoryRef: pane.directoryRef, expectedRevision: pane.endpoint.revision, cursor, pageSize: 256 });
      if (listing.directoryRef === pane.directoryRef
        && listing.revision === pane.endpoint.revision
        && pane.endpointRevision === endpointRevision
        && pane.directoryRevision === directoryRevision) {
        pane.entries.push(...listing.entries.map(mapLocalEntry));
        nextCursorByPane[pane.paneId] = listing.nextCursor;
      }
    } else if (pane.endpoint.kind === "remote" && pane.endpoint.sessionId && pane.endpoint.generation) {
      const listing = await listSftpDirectory({ sessionId: pane.endpoint.sessionId, expectedGeneration: pane.endpoint.generation, path: remotePath(pane.remoteDirectoryPathBytes ?? [47]), cursor, pageSize: 256 });
      if (listing.sessionId === pane.endpoint.sessionId
        && listing.generation === pane.endpoint.generation
        && pane.endpointRevision === endpointRevision
        && pane.directoryRevision === directoryRevision) {
        pane.entries.push(...listing.entries.map(mapRemoteEntry));
        nextCursorByPane[pane.paneId] = listing.nextCursor;
      } else if (listing.nextCursor) {
        await cancelSftpDirectoryListing({ sessionId: listing.sessionId, expectedGeneration: listing.generation, path: listing.path, cursor: listing.nextCursor }).catch(() => undefined);
      }
    }
  } catch { pane.error = t("sftp.paneLoadFailed") } finally { if (pane.endpointRevision === endpointRevision) pane.loading = false }
}
async function chooseLocalFolder(pane: SftpPaneState) {
  if (paneInteractionPending(pane.paneId)) return;
  const endpointRevision = pane.endpointRevision;
  pendingPaneIds.add(pane.paneId);
  try {
    const selected = await open({ directory: true, multiple: false, title: t("sftp.chooseLocalFolder") });
    if (!selected || Array.isArray(selected) || !paneStillActive(pane, endpointRevision)) return;
    const capability = await registerSftpLocalDirectory(selected);
    let adopted = false;
    try {
      const listing = await firstLocalDirectoryPage(capability);
      if (!paneStillActive(pane, endpointRevision)) return;
      await releaseLocalPane(pane.paneId);
      if (!paneStillActive(pane, endpointRevision)) return;
      localTrailByPane.set(pane.paneId, []);
      setRememberableLocalPath(pane, capability);
      replaceSftpPaneEndpoint(pane, { kind: "local", directoryRef: capability.directoryRef, revision: capability.revision, displayPath: selected }, selected, null, capability.directoryRef);
      adopted = true;
      applyFirstLocalDirectoryPage(pane, listing);
    } finally {
      if (!adopted) await releaseLocalCapability(capability);
    }
  } catch {
    if (paneStillActive(pane, endpointRevision)) pane.error = t("sftp.paneLoadFailed");
  } finally {
    pendingPaneIds.delete(pane.paneId);
  }
}

async function openTypedPath(pane: SftpPaneState) {
  const requested = (pathDraftByPane[pane.paneId] ?? pane.directory).trim();
  if (!requested || paneInteractionPending(pane.paneId)) {
    pathDraftByPane[pane.paneId] = pane.directory;
    return;
  }
  if (pane.endpoint.kind === "remote") {
    if (!paneReady(pane)) return;
    await loadRemoteDirectory(pane, Array.from(encoder.encode(requested)));
    pathDraftByPane[pane.paneId] = pane.directory;
    return;
  }
  pendingPaneIds.add(pane.paneId);
  const endpointRevision = pane.endpointRevision;
  try {
    const homeRelative = requested === "~" || requested.startsWith("~/") || requested.startsWith("~\\");
    const expandedPath = homeRelative
      ? (() => {
        const separator = sep();
        const relativePath = requested.slice(1).replace(/^[\\/]+/, "").replace(/[\\/]+/g, separator);
        return homeDir().then((home) => {
          const normalizedHome = home.replace(/[\\/]+$/, "");
          return relativePath ? `${normalizedHome}${separator}${relativePath}` : normalizedHome;
        });
      })()
      : Promise.resolve(requested);
    const resolvedPath = await expandedPath;
    const capability = await registerSftpLocalDirectory(resolvedPath);
    let adopted = false;
    try {
      const listing = await firstLocalDirectoryPage(capability);
      if (!paneStillActive(pane, endpointRevision)) return;
      await releaseLocalPane(pane.paneId);
      if (!paneStillActive(pane, endpointRevision)) return;
      localTrailByPane.set(pane.paneId, []);
      setRememberableLocalPath(pane, capability);
      replaceSftpPaneEndpoint(
        pane,
        { kind: "local", directoryRef: capability.directoryRef, revision: capability.revision, displayPath: requested },
        requested,
        null,
        capability.directoryRef,
      );
      adopted = true;
      applyFirstLocalDirectoryPage(pane, listing);
    } finally {
      if (!adopted) await releaseLocalCapability(capability);
    }
  } catch {
    if (paneStillActive(pane, endpointRevision)) {
      pane.error = t("sftp.pathOpenFailed");
      pathDraftByPane[pane.paneId] = pane.directory;
    }
  } finally {
    pendingPaneIds.delete(pane.paneId);
  }
}

function localDefaultDisplayPath() {
  return sep() === "\\" ? "C:\\" : "~/";
}

function joinLocalDisplayPath(parent: string, child: string) {
  const separator = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
  return parent.endsWith(separator) ? `${parent}${child}` : `${parent}${separator}${child}`;
}

async function initializeDefaultLocalPane(pane: SftpPaneState, useRememberedDirectory = true) {
  if (!canUseDesktopCore() || pane.endpoint.kind !== "local" || pane.endpoint.directoryRef) return;
  const endpointRevision = pane.endpointRevision;
  if (!paneStillActive(pane, endpointRevision)) return;
  const defaultDisplayPath = localDefaultDisplayPath();
  const rememberedPath = useRememberedDirectory ? browserPreferences.rememberedLocalDirectory() : null;
  const displayPath = rememberedPath ?? defaultDisplayPath;
  const selectedPath = rememberedPath ?? (defaultDisplayPath === "C:\\" ? defaultDisplayPath : await homeDir());
  const capability = await registerSftpLocalDirectory(selectedPath);
  if (!paneStillActive(pane, endpointRevision) || pane.endpoint.kind !== "local" || pane.endpoint.directoryRef) {
    await releaseLocalCapability(capability);
    return;
  }
  localTrailByPane.set(pane.paneId, []);
  setRememberableLocalPath(pane, capability);
  replaceSftpPaneEndpoint(
    pane,
    {
      kind: "local",
      directoryRef: capability.directoryRef,
      revision: capability.revision,
      displayPath,
    },
    displayPath,
    null,
    capability.directoryRef,
  );
  const loaded = await loadLocalDirectory(pane);
  if (!paneStillActive(pane, endpointRevision + 1) || pane.endpoint.kind !== "local"
    || pane.endpoint.directoryRef !== capability.directoryRef || pane.endpoint.revision !== capability.revision) {
    await releaseLocalCapability(capability);
    return;
  }
  if (loaded) return;
  await releaseLocalCapability(capability);
  localTrailByPane.set(pane.paneId, []);
  localFilesystemPathByPane.delete(pane.paneId);
  replaceSftpPaneEndpoint(pane, { kind: "local", directoryRef: null, revision: null, displayPath: "" }, ".", null, null);
  if (!rememberedPath) {
    pane.error = t("sftp.paneLoadFailed");
    return;
  }
  showDirectoryMemoryRestoreFailed();
  await initializeDefaultLocalPane(pane, false);
}

async function initializeDefaultLocalPaneSafely(pane: SftpPaneState) {
  try {
    await initializeDefaultLocalPane(pane);
  } catch {
    if (!sftpViewMounted) return;
    if (pane.endpoint.kind !== "local" || pane.endpoint.directoryRef) return;
    if (browserPreferences.rememberedLocalDirectory()) {
      showDirectoryMemoryRestoreFailed();
      try {
        await initializeDefaultLocalPane(pane, false);
        return;
      } catch { /* fall through to the normal initial-directory error. */ }
    }
    if (pane.endpoint.kind === "local" && !pane.endpoint.directoryRef) pane.error = t("sftp.paneLoadFailed");
  }
}
async function activateEntry(pane: SftpPaneState, entry: SftpPaneEntry) {
  if (paneInteractionPending(pane.paneId)) return;
  if (entry.kind !== "directory") { selectSftpPaneEntry(pane, entry.key); return }
  if (pane.endpoint.kind === "remote" && entry.remotePathBytes) { await loadRemoteDirectory(pane, entry.remotePathBytes); return }
  if (pane.endpoint.kind === "local" && pane.endpoint.directoryRef && pane.endpoint.revision) {
    const endpointRevision = pane.endpointRevision;
    pendingPaneIds.add(pane.paneId);
    try {
      const child = await openSftpLocalDirectoryChild({ parentDirectoryRef: pane.endpoint.directoryRef, expectedParentRevision: pane.endpoint.revision, entryRef: entry.entryRef });
      let adopted = false;
      try {
        const listing = await firstLocalDirectoryPage(child);
        if (!paneStillActive(pane, endpointRevision)) return;
        const trail = localTrailByPane.get(pane.paneId) ?? [];
        trail.push({
          capability: {
            directoryRef: pane.endpoint.directoryRef,
            revision: pane.endpoint.revision,
            displayName: pane.endpoint.displayPath,
            rememberablePath: localFilesystemPathByPane.get(pane.paneId) ?? null,
          },
          displayPath: pane.directory,
          rememberedPath: localFilesystemPathByPane.get(pane.paneId) ?? null,
        });
        localTrailByPane.set(pane.paneId, trail);
        setRememberableLocalPath(pane, child);
        replaceSftpPaneEndpoint(pane, { kind: "local", directoryRef: child.directoryRef, revision: child.revision, displayPath: child.displayName }, joinLocalDisplayPath(pane.directory, child.displayName), null, child.directoryRef);
        adopted = true;
        applyFirstLocalDirectoryPage(pane, listing);
      } finally {
        if (!adopted) await releaseLocalCapability(child);
      }
    } catch {
      if (paneStillActive(pane, endpointRevision)) pane.error = t("sftp.localDirectoryOpenFailed");
    } finally {
      pendingPaneIds.delete(pane.paneId);
    }
  }
}
async function goUp(pane: SftpPaneState) {
  if (paneInteractionPending(pane.paneId)) return;
  if (pane.endpoint.kind === "remote") { await loadRemoteDirectory(pane, parentRemoteBytes(pane.remoteDirectoryPathBytes ?? [47])); return }
  const trail = localTrailByPane.get(pane.paneId) ?? [];
  const parent = trail.pop();
  if (!parent || !pane.endpoint.directoryRef || !pane.endpoint.revision) return;
  const current = { directoryRef: pane.endpoint.directoryRef, expectedRevision: pane.endpoint.revision };
  localTrailByPane.set(pane.paneId, trail);
  if (parent.rememberedPath) localFilesystemPathByPane.set(pane.paneId, parent.rememberedPath);
  else localFilesystemPathByPane.delete(pane.paneId);
  replaceSftpPaneEndpoint(pane, { kind: "local", directoryRef: parent.capability.directoryRef, revision: parent.capability.revision, displayPath: parent.capability.displayName }, parent.displayPath, null, parent.capability.directoryRef);
  await releaseSftpLocalDirectory(current).catch(() => undefined);
  await loadLocalDirectory(pane);
}

async function uploadFilesFromPicker(target: SftpPaneState) {
  if (!paneReady(target) || target.endpoint.kind !== "remote"
    || !target.endpoint.sessionId || !target.endpoint.generation
    || paneInteractionPending(target.paneId)) return;
  pendingPaneIds.add(target.paneId);
  try {
    const selected = await open({ multiple: true, directory: false, title: t("sftp.chooseUpload") });
    const paths = selected === null ? [] : Array.isArray(selected) ? selected : [selected];
    await uploadLocalFiles(target, paths, true);
  } catch { showOperationFailed(); } finally { pendingPaneIds.delete(target.paneId) }
}

async function uploadLocalFiles(target: SftpPaneState, paths: string[], paneAlreadyPending = false) {
  if (!paths.length || !paneReady(target) || target.endpoint.kind !== "remote"
    || !target.endpoint.sessionId || !target.endpoint.generation
    || (!paneAlreadyPending && paneInteractionPending(target.paneId))) return;
  const targetPaneId = target.paneId;
  const targetEndpointRevision = target.endpointRevision;
  const targetDirectoryRevision = target.directoryRevision;
  const sessionId = target.endpoint.sessionId;
  const generation = target.endpoint.generation;
  const directoryPathBytes = [...(target.remoteDirectoryPathBytes ?? [47])];
  if (!paneAlreadyPending) pendingPaneIds.add(targetPaneId);
  let replaceRemaining = false;
  try {
    for (const path of paths) {
      const boundary = await registerSftpLocalBoundary("uploadSource", path);
      const current = paneStates[targetPaneId];
      if (current !== target
        || current.endpointRevision !== targetEndpointRevision
        || current.directoryRevision !== targetDirectoryRevision
        || !paneReady(current)) {
        target.error = t("sftp.paneFenceChanged");
        return;
      }
      const existingTarget = matchingTargetEntry(current, boundary.displayName);
      let conflictPolicy: SftpConflictPolicy = "failIfExists";
      if (existingTarget) {
        const decision = replaceRemaining ? "replaceAll" : await requestOverwriteConfirmation(
          boundary.displayName,
          existingTarget.kind === "directory" ? "directory" : "file",
          paths.length > 1,
        );
        if (decision === "skip") continue;
        if (current !== target
          || current.endpointRevision !== targetEndpointRevision
          || current.directoryRevision !== targetDirectoryRevision
          || !paneReady(current)) {
          target.error = t("sftp.paneFenceChanged");
          return;
        }
        if (decision === "replaceAll") replaceRemaining = true;
        conflictPolicy = "replaceSafely";
      }
      const transfer = await enqueueSftpTransfer({
        sessionId,
        expectedGeneration: generation,
        direction: "upload",
        source: { kind: "localBoundaryToken", token: boundary.token, displayName: boundary.displayName },
        target: { kind: "remote", path: remotePath(joinRemoteBytes(directoryPathBytes, boundary.displayName)) },
        expectedBytes: boundary.size ?? 0,
        conflictPolicy,
      });
      rememberTransferTarget(transfer.transferId, target);
    }
    await refreshSnapshot();
  } catch {
    target.error = t("sftp.localFileUploadFailed");
    showOperationFailed();
  } finally {
    if (!paneAlreadyPending) pendingPaneIds.delete(targetPaneId);
  }
}

async function downloadSelectedFromPicker(pane: SftpPaneState) {
  const entry = pane.entries.find((candidate) => candidate.key === pane.selectedEntryKey);
  if (pane.endpoint.kind !== "remote" || !pane.endpoint.sessionId || !pane.endpoint.generation
    || !entry || entry.kind !== "file" || entry.size === null || !entry.remotePathBytes
    || !paneReady(pane) || paneInteractionPending(pane.paneId)) return;
  const endpointRevision = pane.endpointRevision;
  const directoryRevision = pane.directoryRevision;
  const selectionRevision = pane.selectionRevision;
  const sessionId = pane.endpoint.sessionId;
  const generation = pane.endpoint.generation;
  const remotePathBytes = [...entry.remotePathBytes];
  pendingPaneIds.add(pane.paneId);
  try {
    const selected = await save({ title: t("sftp.chooseDownload"), defaultPath: entry.displayName });
    if (!selected) return;
    if (pane.endpointRevision !== endpointRevision
      || pane.directoryRevision !== directoryRevision
      || pane.selectionRevision !== selectionRevision
      || pane.selectedEntryKey !== entry.key
      || !paneReady(pane)) {
      pane.error = t("sftp.paneFenceChanged");
      return;
    }
    const boundary = await registerSftpLocalBoundary("downloadTarget", selected);
    if (pane.endpointRevision !== endpointRevision
      || pane.directoryRevision !== directoryRevision
      || pane.selectionRevision !== selectionRevision
      || pane.selectedEntryKey !== entry.key
      || !paneReady(pane)) {
      pane.error = t("sftp.paneFenceChanged");
      return;
    }
    await enqueueSftpTransfer({
      sessionId,
      expectedGeneration: generation,
      direction: "download",
      source: { kind: "remote", path: remotePath(remotePathBytes) },
      target: { kind: "localBoundaryToken", token: boundary.token, displayName: boundary.displayName },
      expectedBytes: entry.size,
      conflictPolicy: "replaceSafely",
    });
    await refreshSnapshot();
  } catch { showOperationFailed(); } finally {
    pendingPaneIds.delete(pane.paneId);
  }
}

function previewKindForEntry(entry: SftpPaneEntry | null): "text" | "image" | null {
  if (!entry || entry.kind !== "file" || entry.size === null) return null;
  const lowerName = entry.displayName.toLocaleLowerCase("en-US");
  const baseName = lowerName.split("/").at(-1) ?? lowerName;
  const extension = baseName.includes(".") ? baseName.split(".").at(-1) : null;
  if (previewImageExtensions.has(extension ?? "")) {
    return entry.size <= 8 * 1024 * 1024 ? "image" : null;
  }
  return previewTextExtensions.has(extension ?? "")
    || previewTextNames.has(baseName)
    || isNginxStandardSiteFile(entry)
    ? "text"
    : null;
}

function isNginxStandardSiteFile(entry: SftpPaneEntry): boolean {
  const path = entry.remotePathBytes;
  if (!path) return false;
  for (const prefix of nginxSiteDirectoryPrefixes) {
    if (!prefix.every((byte, index) => path[index] === byte)) continue;
    const name = path.slice(prefix.length);
    if (!name.length || name.some((byte) => byte === 47 || byte <= 31 || byte === 127)) continue;
    if (name.length === 1 && name[0] === 46) continue;
    if (name.length === 2 && name[0] === 46 && name[1] === 46) continue;
    return true;
  }
  return false;
}

function releasePreviewImage() {
  if (previewImageUrl.value) URL.revokeObjectURL(previewImageUrl.value);
  previewImageUrl.value = null;
}

function closeFilePreview() {
  previewRequestRevision += 1;
  if (previewTailTimer !== null) window.clearTimeout(previewTailTimer);
  previewTailTimer = null;
  previewOpen.value = false;
  previewLoading.value = false;
  previewContent.value = null;
  previewError.value = false;
  previewText.value = "";
  previewOriginalText.value = "";
  previewTextEditable.value = false;
  previewTextLineEnding.value = "lf";
  previewTailActive.value = false;
  previewTailUsed.value = false;
  previewTailOffset.value = "0";
  previewTailError.value = false;
  previewTailReset.value = false;
  previewTailTrimmed.value = false;
  previewSaving.value = false;
  previewSaveError.value = false;
  previewTarget.value = null;
  previewMode.value = "preview";
  previewTailDecoder = new TextDecoder();
  previewTailPendingCarriageReturn = false;
  releasePreviewImage();
}

function textLineEnding(value: SftpTextLineEnding): "\n" | "\r\n" | "\r" {
  if (value === "crLf") return "\r\n";
  if (value === "cr") return "\r";
  return "\n";
}

function normalizeEditorText(value: string) {
  return value.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
}

function normalizeTailText(value: string, reset = false) {
  if (reset) previewTailPendingCarriageReturn = false;
  let combined = previewTailPendingCarriageReturn ? `\r${value}` : value;
  previewTailPendingCarriageReturn = combined.endsWith("\r");
  if (previewTailPendingCarriageReturn) combined = combined.slice(0, -1);
  return normalizeEditorText(combined);
}

function currentPreviewPane(target: TextPreviewTarget | null): SftpPaneState | null {
  if (!target) return null;
  const pane = paneStates[target.paneId];
  if (!pane || pane.endpoint.kind !== "remote"
    || pane.endpointRevision !== target.endpointRevision
    || pane.directoryRevision !== target.directoryRevision
    || pane.selectionRevision !== target.selectionRevision
    || pane.endpoint.sessionId !== target.sessionId
    || pane.endpoint.generation !== target.generation
    || pane.directoryRef !== target.directoryRef
    || pane.selectedEntryKey !== target.entryKey
    || !paneReady(pane)) return null;
  return pane;
}

function schedulePreviewTail(delay = 1000) {
  if (!previewOpen.value || !previewTailActive.value || previewTailTimer !== null) return;
  previewTailTimer = window.setTimeout(() => {
    previewTailTimer = null;
    void pollPreviewTail();
  }, delay);
}

async function pollPreviewTail() {
  const target = previewTarget.value;
  const requestRevision = previewRequestRevision;
  if (!previewTailActive.value || !currentPreviewPane(target) || !target) return;
  try {
    const result = await tailSftpFile({
      sessionId: target.sessionId,
      expectedGeneration: target.generation,
      directoryRef: target.directoryRef,
      entryRef: target.entryRef,
      offset: previewTailOffset.value,
    });
    if (requestRevision !== previewRequestRevision || !previewTailActive.value || !currentPreviewPane(target)) return;
    if (result.reset) {
      previewTailDecoder = new TextDecoder();
      previewText.value = normalizeTailText(
        previewTailDecoder.decode(new Uint8Array(result.bytes), { stream: true }),
        true,
      );
      previewTailReset.value = true;
    } else if (result.bytes.length) {
      previewText.value += normalizeTailText(
        previewTailDecoder.decode(new Uint8Array(result.bytes), { stream: true }),
      );
    }
    if (previewText.value.length > maximumTailEditorCharacters) {
      previewText.value = previewText.value.slice(-maximumTailEditorCharacters);
      previewTailTrimmed.value = true;
    }
    previewTailOffset.value = result.nextOffset;
    previewTailError.value = false;
    schedulePreviewTail(BigInt(result.nextOffset) < BigInt(result.totalSize) ? 40 : 1000);
  } catch {
    if (requestRevision === previewRequestRevision) {
      previewTailError.value = true;
      previewTailActive.value = false;
    }
  }
}

function togglePreviewTail() {
  if (previewTailActive.value) {
    previewTailActive.value = false;
    if (previewTailTimer !== null) window.clearTimeout(previewTailTimer);
    previewTailTimer = null;
    return;
  }
  previewTailActive.value = true;
  previewTailUsed.value = true;
  previewTailError.value = false;
  schedulePreviewTail(0);
}

async function savePreviewText() {
  const target = previewTarget.value;
  const pane = currentPreviewPane(target);
  if (!target || !pane || !previewTextEditable.value || previewTailUsed.value
    || !previewDirty.value || previewSaveTooLarge.value || previewSaving.value) return;
  previewSaving.value = true;
  previewSaveError.value = false;
  try {
    await mutateSftpFile({
      sessionId: target.sessionId,
      expectedGeneration: target.generation,
      mutation: {
        kind: "writeText",
        path: remotePath(target.pathBytes),
        precondition: target.precondition,
        text: previewText.value.replace(/\n/g, textLineEnding(previewTextLineEnding.value)),
      },
    });
    previewOriginalText.value = previewText.value;
    closeFilePreview();
    await loadRemoteDirectory(pane);
  } catch {
    previewSaveError.value = true;
  } finally {
    previewSaving.value = false;
  }
}

async function previewSelectedFile(
  pane: SftpPaneState,
  mode: "preview" | "tail" = "preview",
  allowPanePending = false,
) {
  const entry = pane.entries.find((candidate) => candidate.key === pane.selectedEntryKey) ?? null;
  if (pane.endpoint.kind !== "remote" || !pane.endpoint.sessionId || !pane.endpoint.generation
    || !pane.directoryRef || !entry || !entry.remotePathBytes || !previewKindForEntry(entry)
    || !paneReady(pane) || (!allowPanePending && paneInteractionPending(pane.paneId))) return;
  if (canUseDesktopCore()) {
    const hostId = pane.endpoint.hostId;
    try {
      await openToolWindow({
        kind: "sftpFile", title: `${hosts.value.find((host) => host.hostId === hostId)?.label ?? "SFTP"} · ${entry.displayName}`,
        request: { meta: { requestId: crypto.randomUUID() }, sessionId: pane.endpoint.sessionId, expectedGeneration: pane.endpoint.generation, directoryRef: pane.directoryRef, entryRef: entry.entryRef },
        path: remotePath(entry.remotePathBytes), precondition: { ...entry.precondition }, tail: mode === "tail",
      });
    } catch { showOperationFailed(); }
    return;
  }
  const requestRevision = ++previewRequestRevision;
  const endpointRevision = pane.endpointRevision;
  const directoryRevision = pane.directoryRevision;
  const selectionRevision = pane.selectionRevision;
  previewName.value = entry.displayName;
  previewMode.value = mode;
  previewContent.value = null;
  previewError.value = false;
  previewSaveError.value = false;
  releasePreviewImage();
  previewOpen.value = true;
  previewLoading.value = true;
  try {
    const response = await previewSftpFile({
      sessionId: pane.endpoint.sessionId,
      expectedGeneration: pane.endpoint.generation,
      directoryRef: pane.directoryRef,
      entryRef: entry.entryRef,
    });
    if (requestRevision !== previewRequestRevision
      || pane.endpointRevision !== endpointRevision
      || pane.directoryRevision !== directoryRevision
      || pane.selectionRevision !== selectionRevision
      || pane.selectedEntryKey !== entry.key
      || !paneReady(pane)) return;
    previewContent.value = response.content;
    if (response.content.kind === "text") {
      const normalizedText = normalizeEditorText(response.content.text);
      previewText.value = normalizedText;
      previewOriginalText.value = normalizedText;
      previewTextEditable.value = response.content.editable;
      previewTextLineEnding.value = response.content.lineEnding;
      previewTailOffset.value = response.content.endOffset;
      previewTarget.value = {
        paneId: pane.paneId,
        endpointRevision,
        directoryRevision,
        selectionRevision,
        sessionId: pane.endpoint.sessionId,
        generation: pane.endpoint.generation,
        directoryRef: pane.directoryRef,
        entryRef: entry.entryRef,
        entryKey: entry.key,
        pathBytes: [...(entry.remotePathBytes ?? [])],
        precondition: { ...entry.precondition },
      };
      if (mode === "tail") {
        previewTailUsed.value = true;
        togglePreviewTail();
      }
    } else {
      previewImageUrl.value = URL.createObjectURL(new Blob(
        [new Uint8Array(response.content.bytes)],
        { type: response.content.mediaType },
      ));
    }
  } catch {
    if (requestRevision === previewRequestRevision) previewError.value = true;
  } finally {
    if (requestRevision === previewRequestRevision) previewLoading.value = false;
  }
}

function openMutation(kind: "mkdir" | "touch" | "rename" | "delete") {
  const pane = activePane.value;
  if (!pane || pane.endpoint.kind !== "remote" || paneInteractionPending(pane.paneId)) return;
  const selectedEntry = pane.entries.find((entry) => entry.key === pane.selectedEntryKey) ?? null;
  const selectedEntries = pane.entries.filter((entry) => pane.selectedEntryKeys.includes(entry.key));
  if ((kind === "rename" || kind === "delete") && !selectedEntry) return;
  if (kind === "rename" && selectedEntries.length !== 1) return;
  mutationTargetPaneId.value = pane.paneId;
  mutationDialog.value = kind;
  mutationName.value = kind === "rename" ? selectedEntry?.displayName ?? "" : "";
}

function openPermissions(pane: SftpPaneState) {
  const entry = pane.entries.find((candidate) => candidate.key === pane.selectedEntryKey);
  if (pane.endpoint.kind !== "remote" || !pane.endpoint.sessionId || !pane.endpoint.generation
    || !entry?.remotePathBytes || !["file", "directory"].includes(entry.kind)
    || entry.permissionBits == null || pane.selectedEntryKeys.length !== 1
    || paneInteractionPending(pane.paneId)) return;
  permissionsTarget.value = {
    paneId: pane.paneId,
    endpointRevision: pane.endpointRevision,
    directoryRevision: pane.directoryRevision,
    sessionId: pane.endpoint.sessionId,
    generation: pane.endpoint.generation,
    entryKey: entry.key,
    displayName: entry.displayName,
    pathBytes: [...entry.remotePathBytes],
    precondition: entry.precondition,
    expectedPermissionBits: entry.permissionBits,
  };
  permissionMode.value = entry.permissionBits & 0o777;
}

function setPermissionBit(bit: number, enabled: boolean) {
  permissionMode.value = enabled ? permissionMode.value | bit : permissionMode.value & ~bit;
}

async function applyPermissions() {
  const target = permissionsTarget.value;
  if (!target || permissionsPending.value) return;
  const pane = paneStates[target.paneId];
  const entry = pane?.entries.find((candidate) => candidate.key === target.entryKey);
  if (!pane || pane.endpoint.kind !== "remote" || pane.endpoint.sessionId !== target.sessionId
    || pane.endpoint.generation !== target.generation || pane.endpointRevision !== target.endpointRevision
    || pane.directoryRevision !== target.directoryRevision || !paneReady(pane)
    || !entry || entry.permissionBits !== target.expectedPermissionBits
    || paneInteractionPending(pane.paneId)) {
    if (pane) pane.error = t("sftp.paneFenceChanged");
    permissionsTarget.value = null;
    return;
  }
  permissionsPending.value = true;
  pendingPaneIds.add(pane.paneId);
  try {
    await mutateSftpFile({
      sessionId: target.sessionId,
      expectedGeneration: target.generation,
      mutation: {
        kind: "setPermissions",
        path: remotePath(target.pathBytes),
        precondition: target.precondition,
        expectedPermissionBits: target.expectedPermissionBits,
        mode: permissionMode.value,
      },
    });
    permissionsTarget.value = null;
    await loadRemoteDirectory(pane, pane.remoteDirectoryPathBytes ?? [47], true);
  } catch {
    showOperationFailed();
  } finally {
    pendingPaneIds.delete(pane.paneId);
    permissionsPending.value = false;
  }
}

function openFileUtility(kind: "compress" | "extract" | "downloadUrl") {
  const pane = activePane.value;
  if (!pane || pane.endpoint.kind !== "remote" || paneInteractionPending(pane.paneId)) return;
  const selectedEntries = pane.entries.filter((entry) => pane.selectedEntryKeys.includes(entry.key));
  if (kind === "compress") {
    if (!selectedEntries.length
      || selectedEntries.some((entry) => !["file", "directory"].includes(entry.kind))) return;
    fileUtilityName.value = selectedEntries.length === 1
      ? `${selectedEntries[0]?.displayName ?? "archive"}.zip`
      : "archive.zip";
  } else if (kind === "extract") {
    const entry = selectedEntries[0];
    if (selectedEntries.length !== 1 || entry?.kind !== "file"
      || !entry.displayName.toLowerCase().endsWith(".zip")) return;
    fileUtilityName.value = entry.displayName.replace(/\.zip$/i, "") || "archive";
  } else {
    fileUtilityName.value = "download";
    fileUtilityUrl.value = "https://";
  }
  fileUtilityTargetPaneId.value = pane.paneId;
  fileUtilityDialog.value = kind;
}

function inferNetworkFileName() {
  try {
    const url = new URL(fileUtilityUrl.value.trim());
    const segment = decodeURIComponent(url.pathname.split("/").filter(Boolean).at(-1) ?? "");
    if (segment && segment !== "." && segment !== ".." && !segment.includes("/") && !segment.includes("\\")) {
      fileUtilityName.value = segment;
    }
  } catch { /* keep the current explicit target name */ }
}

function refreshPane(pane: SftpPaneState) {
  if (!paneReady(pane) || paneInteractionPending(pane.paneId)) return;
  void (pane.endpoint.kind === "local" ? loadLocalDirectory(pane) : loadRemoteDirectory(pane));
}

function openSftpContextMenu(event: MouseEvent, pane: SftpPaneState, entry?: SftpPaneEntry) {
  if (pane.endpoint.kind !== "remote" || !paneReady(pane) || paneInteractionPending(pane.paneId)) return;
  event.preventDefault();
  event.stopPropagation();
  if (entry && !pane.selectedEntryKeys.includes(entry.key)) selectSftpPaneEntry(pane, entry.key);
  activeSftpPaneId.value = pane.paneId;
  contextMenu.value = {
    paneId: pane.paneId,
    anchorX: event.clientX,
    anchorY: event.clientY,
    left: event.clientX,
    top: event.clientY,
  };
}

// Measure the rendered menu, including translated labels and conditional actions.
// CSS bounds oversized menus; placement then keeps the whole scrollable box visible.
function positionContextMenu() {
  const menu = contextMenu.value;
  const element = contextMenuRoot.value;
  if (!menu || !element) return;
  const { width, height } = element.getBoundingClientRect();
  const inset = 8;
  const viewportWidth = document.documentElement.clientWidth || window.innerWidth;
  const viewportHeight = document.documentElement.clientHeight || window.innerHeight;
  menu.left = Math.max(inset, Math.min(menu.anchorX, viewportWidth - width - inset));
  menu.top = Math.max(inset, Math.min(menu.anchorY, viewportHeight - height - inset));
}
watch(contextMenuRoot, (element, _previous, onCleanup) => {
  if (!element) return;
  positionContextMenu();
  const observer = new ResizeObserver(positionContextMenu);
  observer.observe(element);
  window.addEventListener("resize", positionContextMenu);
  onCleanup(() => {
    observer.disconnect();
    window.removeEventListener("resize", positionContextMenu);
  });
}, { flush: "post" });
watch(() => [contextMenu.value?.anchorX, contextMenu.value?.anchorY], positionContextMenu, { flush: "post" });

function runSftpContextAction(action: "refresh" | "mkdir" | "touch" | "preview" | "tail" | "download" | "rename" | "delete" | "permissions" | "compress" | "extract" | "downloadUrl") {
  const pane = contextMenu.value ? paneStates[contextMenu.value.paneId] : null;
  contextMenu.value = null;
  if (!pane) return;
  activeSftpPaneId.value = pane.paneId;
  if (action === "refresh") refreshPane(pane);
  else if (action === "preview") void previewSelectedFile(pane);
  else if (action === "tail") void previewSelectedFile(pane, "tail");
  else if (action === "download") void downloadSelectedFromPicker(pane);
  else if (action === "permissions") openPermissions(pane);
  else if (action === "compress" || action === "extract" || action === "downloadUrl") openFileUtility(action);
  else openMutation(action);
}

async function performFileUtility() {
  const pane = fileUtilityPane.value;
  const kind = fileUtilityDialog.value;
  if (!pane || pane.endpoint.kind !== "remote" || !pane.endpoint.sessionId || !pane.endpoint.generation
    || !kind || !fileUtilityValid.value || paneInteractionPending(pane.paneId)) return;
  const currentBytes = pane.remoteDirectoryPathBytes ?? [47];
  const target = remotePath(joinRemoteBytes(currentBytes, fileUtilityName.value.trim()));
  const selectedEntries = pane.entries.filter((entry) => pane.selectedEntryKeys.includes(entry.key));
  let mutation;
  if (kind === "compress") {
    const sources = selectedEntries.flatMap((entry) => entry.remotePathBytes ? [{
      path: remotePath(entry.remotePathBytes),
      precondition: entry.precondition,
      archiveName: entry.displayName,
    }] : []);
    if (!sources.length) return;
    mutation = { kind: "createZip" as const, sources, target };
  } else if (kind === "extract") {
    const entry = selectedEntries[0];
    if (!entry?.remotePathBytes) return;
    mutation = {
      kind: "extractZip" as const,
      source: remotePath(entry.remotePathBytes),
      sourcePrecondition: entry.precondition,
      targetDirectory: target,
    };
  } else {
    mutation = { kind: "downloadUrl" as const, url: fileUtilityUrl.value.trim(), target };
  }
  pendingPaneIds.add(pane.paneId);
  let shouldRefresh = false;
  try {
    await mutateSftpFile({
      sessionId: pane.endpoint.sessionId,
      expectedGeneration: pane.endpoint.generation,
      mutation,
    });
    fileUtilityDialog.value = null;
    fileUtilityTargetPaneId.value = null;
    shouldRefresh = true;
  } catch (error) { showFileUtilityFailed(error); } finally { pendingPaneIds.delete(pane.paneId) }
  if (shouldRefresh) await loadRemoteDirectory(pane);
}

async function performMutation() {
  const pane = mutationPane.value;
  const entry = pane?.entries.find((candidate) => candidate.key === pane.selectedEntryKey) ?? null;
  const kind = mutationDialog.value;
  if (!pane || pane.endpoint.kind !== "remote" || !pane.endpoint.sessionId || !pane.endpoint.generation
    || !kind || paneInteractionPending(pane.paneId)) return;
  const currentBytes = pane.remoteDirectoryPathBytes ?? [47];
  const mutation = kind === "mkdir"
    ? { kind: "createDirectory" as const, path: remotePath(joinRemoteBytes(currentBytes, mutationName.value.trim())) }
    : kind === "touch"
      ? { kind: "createEmptyFile" as const, path: remotePath(joinRemoteBytes(currentBytes, mutationName.value.trim())) }
    : kind === "rename" && entry?.remotePathBytes
      ? { kind: "renameNoReplace" as const, source: remotePath(entry.remotePathBytes), target: remotePath(joinRemoteBytes(currentBytes, mutationName.value.trim())), sourcePrecondition: entry.precondition }
    : null;
  const deleteEntries = kind === "delete"
    ? pane.entries.filter((candidate) => pane.selectedEntryKeys.includes(candidate.key) && candidate.remotePathBytes)
    : [];
  if ((kind === "delete" ? deleteEntries.length === 0 : !mutation)
    || (kind !== "delete" && !mutationNameValid.value)) return;
  pendingPaneIds.add(pane.paneId);
  let shouldRefresh = false;
  try {
    if (kind === "delete") {
      for (const candidate of deleteEntries) {
        await mutateSftpFile({
          sessionId: pane.endpoint.sessionId,
          expectedGeneration: pane.endpoint.generation,
          mutation: {
            kind: "delete",
            path: remotePath(candidate.remotePathBytes!),
            precondition: candidate.precondition,
            irreversibleConfirmed: true,
          },
        });
      }
    } else {
      await mutateSftpFile({ sessionId: pane.endpoint.sessionId, expectedGeneration: pane.endpoint.generation, mutation: mutation! });
    }
    mutationDialog.value = null;
    mutationTargetPaneId.value = null;
    shouldRefresh = true;
  } catch { showOperationFailed(); } finally { pendingPaneIds.delete(pane.paneId) }
  if (shouldRefresh) await loadRemoteDirectory(pane);
}

function selectEntryFromClick(event: MouseEvent, pane: SftpPaneState, entry: SftpPaneEntry) {
  if (event.shiftKey) {
    selectSftpPaneEntryRange(
      pane,
      entry.key,
      visibleSftpPaneEntries(pane).map((candidate) => candidate.key),
    );
  } else if (event.metaKey || event.ctrlKey) {
    toggleSftpPaneEntry(pane, entry.key);
  } else {
    selectSftpPaneEntry(pane, entry.key);
  }
}

function beginPointerDrag(event: PointerEvent, pane: SftpPaneState, entry: SftpPaneEntry) {
  if (event.button !== 0 || paneInteractionPending(pane.paneId)) return;
  // Modified-click selection owns the interaction; do not reset the selection or range anchor through the drag entry point first.
  if (event.shiftKey || event.metaKey || event.ctrlKey) return;
  if (!pane.selectedEntryKeys.includes(entry.key)) selectSftpPaneEntry(pane, entry.key);
  pointerDrag.value = {
    pointerId: event.pointerId,
    sourcePaneId: pane.paneId,
    entryKey: entry.key,
    originX: event.clientX,
    originY: event.clientY,
    intent: null,
  };
  (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
}
function pointerDropTarget(event: PointerEvent, intent: SftpPaneDragIntent): SftpPaneState | null {
  const element = document.elementFromPoint(event.clientX, event.clientY);
  const paneId = element?.closest<HTMLElement>("[data-pane-id]")?.dataset.paneId;
  const pane = paneId ? paneStates[paneId] : null;
  if (!pane || paneInteractionPending(pane.paneId)
    || !resolveSftpPaneDropFence(intent, paneStates, pane.paneId)) return null;
  return pane;
}
function updatePointerDrag(event: PointerEvent) {
  const tracking = pointerDrag.value;
  if (!tracking || tracking.pointerId !== event.pointerId) return;
  if (!tracking.intent) {
    const distance = Math.hypot(event.clientX - tracking.originX, event.clientY - tracking.originY);
    if (distance < pointerDragThreshold) return;
    const source = paneStates[tracking.sourcePaneId];
    const entry = source?.entries.find((candidate) => candidate.key === tracking.entryKey);
    if (!source || !entry) {
      pointerDrag.value = null;
      return;
    }
    const intent = captureSftpPaneDragIntent(source, entry);
    if (!intent) {
      pointerDrag.value = null;
      return;
    }
    tracking.intent = intent;
    dragIntent.value = intent;
  }
  event.preventDefault();
  dropPaneId.value = pointerDropTarget(event, tracking.intent)?.paneId ?? null;
}
function finishPointerDrag(event: PointerEvent) {
  const tracking = pointerDrag.value;
  if (!tracking || tracking.pointerId !== event.pointerId) return;
  pointerDrag.value = null;
  const intent = tracking.intent;
  if (!intent) return;
  const target = pointerDropTarget(event, intent);
  dropPaneId.value = null;
  if (!target) {
    dragIntent.value = null;
    return;
  }
  void commitDrop(target);
}
function cancelPointerDrag(event: PointerEvent) {
  if (pointerDrag.value?.pointerId !== event.pointerId) return;
  pointerDrag.value = null;
  dragIntent.value = null;
  dropPaneId.value = null;
}

function nativeDropPane(position: { x: number; y: number }): SftpPaneState | null {
  const scale = window.devicePixelRatio || 1;
  const element = document.elementFromPoint(position.x / scale, position.y / scale);
  const paneId = element?.closest<HTMLElement>("[data-pane-id]")?.dataset.paneId;
  const pane = paneId ? paneStates[paneId] : null;
  if (!pane || pane.endpoint.kind !== "remote" || !paneReady(pane) || paneInteractionPending(pane.paneId)) return null;
  return pane;
}

function handleNativeFileDrop(event: DragDropEvent) {
  if (event.type === "leave") {
    dropPaneId.value = null;
    return;
  }
  const pane = nativeDropPane(event.position);
  dropPaneId.value = pane?.paneId ?? null;
  if (event.type !== "drop") return;
  dropPaneId.value = null;
  if (!pane || !event.paths.length) return;
  activeSftpPaneId.value = pane.paneId;
  void uploadLocalFiles(pane, event.paths);
}

function recursiveEndpointForPane(pane: SftpPaneState): RecursiveDirectoryEndpoint | null {
  if (!pane.directoryRef) return null;
  if (pane.endpoint.kind === "local" && pane.endpoint.revision) {
    return { kind: "local", directoryRef: pane.directoryRef, revision: pane.endpoint.revision };
  }
  if (pane.endpoint.kind === "remote" && pane.endpoint.sessionId && pane.endpoint.generation) {
    return {
      kind: "remote",
      directoryRef: pane.directoryRef,
      sessionId: pane.endpoint.sessionId,
      generation: pane.endpoint.generation,
      pathBytes: [...(pane.remoteDirectoryPathBytes ?? [47])],
    };
  }
  return null;
}

async function listRecursiveDirectory(endpoint: RecursiveDirectoryEndpoint): Promise<RecursiveEntry[]> {
  const entries: RecursiveEntry[] = [];
  let cursor: number[] | null = null;
  do {
    if (endpoint.kind === "local" && endpoint.revision) {
      const listing = await listSftpLocalDirectory({
        directoryRef: endpoint.directoryRef,
        expectedRevision: endpoint.revision,
        cursor,
        pageSize: 256,
      });
      entries.push(...listing.entries.map((entry) => ({
        entryRef: entry.entryRef,
        displayName: entry.displayName,
        kind: entry.kind,
        size: entry.size,
        remotePathBytes: null,
      })));
      cursor = listing.nextCursor;
    } else if (endpoint.kind === "remote" && endpoint.sessionId && endpoint.generation && endpoint.pathBytes) {
      const listing = await listSftpDirectory({
        sessionId: endpoint.sessionId,
        expectedGeneration: endpoint.generation,
        path: remotePath(endpoint.pathBytes),
        cursor,
        pageSize: 256,
      });
      endpoint.directoryRef = listing.directoryRef;
      entries.push(...listing.entries.map((entry) => ({
        entryRef: entry.entryRef,
        displayName: entry.displayName,
        kind: entry.kind,
        size: entry.size,
        remotePathBytes: [...entry.path.bytes],
      })));
      cursor = listing.nextCursor;
    } else {
      throw new Error("invalid recursive SFTP endpoint");
    }
  } while (cursor);
  return entries;
}

async function openRecursiveSourceDirectory(
  parent: RecursiveDirectoryEndpoint,
  entry: RecursiveEntry,
): Promise<RecursiveDirectoryEndpoint> {
  if (parent.kind === "local" && parent.revision) {
    const capability = await openSftpLocalDirectoryChild({
      parentDirectoryRef: parent.directoryRef,
      expectedParentRevision: parent.revision,
      entryRef: entry.entryRef,
    });
    return { kind: "local", directoryRef: capability.directoryRef, revision: capability.revision };
  }
  if (parent.kind === "remote" && parent.sessionId && parent.generation && entry.remotePathBytes) {
    const endpoint: RecursiveDirectoryEndpoint = {
      kind: "remote",
      directoryRef: "",
      sessionId: parent.sessionId,
      generation: parent.generation,
      pathBytes: [...entry.remotePathBytes],
    };
    await listRecursiveDirectory(endpoint);
    return endpoint;
  }
  throw new Error("invalid recursive source directory");
}

async function createRecursiveTargetDirectory(
  parent: RecursiveDirectoryEndpoint,
  name: string,
  existingTarget: RecursiveEntry | null,
): Promise<RecursiveDirectoryEndpoint> {
  if (existingTarget) {
    if (existingTarget.kind !== "directory") throw new Error("recursive target kind mismatch");
    return openRecursiveSourceDirectory(parent, existingTarget);
  }
  if (parent.kind === "local" && parent.revision) {
    const capability = await createSftpLocalDirectoryChild({
      parentDirectoryRef: parent.directoryRef,
      expectedParentRevision: parent.revision,
      name,
    });
    return { kind: "local", directoryRef: capability.directoryRef, revision: capability.revision };
  }
  if (parent.kind === "remote" && parent.sessionId && parent.generation && parent.pathBytes) {
    const pathBytes = joinRemoteBytes(parent.pathBytes, name);
    await mutateSftpFile({
      sessionId: parent.sessionId,
      expectedGeneration: parent.generation,
      mutation: { kind: "createDirectory", path: remotePath(pathBytes) },
    });
    const endpoint: RecursiveDirectoryEndpoint = {
      kind: "remote",
      directoryRef: "",
      sessionId: parent.sessionId,
      generation: parent.generation,
      pathBytes,
    };
    await listRecursiveDirectory(endpoint);
    return endpoint;
  }
  throw new Error("invalid recursive target directory");
}

async function enqueueRecursiveFile(
  source: RecursiveDirectoryEndpoint,
  target: RecursiveDirectoryEndpoint,
  entry: RecursiveEntry,
  fence: ReturnType<typeof resolveSftpPaneDropFence> & {},
  conflictPolicy: SftpConflictPolicy,
) {
  if (entry.size === null) throw new Error("file size unavailable");
  const sourceIntent = source.kind === "local"
    ? { kind: "localDirectoryEntry" as const, directoryRef: source.directoryRef, entryRef: entry.entryRef }
    : { kind: "remoteFile" as const, sessionId: source.sessionId!, expectedGeneration: source.generation!, directoryRef: source.directoryRef, entryRef: entry.entryRef };
  const targetIntent = target.kind === "local"
    ? { kind: "localDirectory" as const, directoryRef: target.directoryRef }
    : { kind: "remoteDirectory" as const, sessionId: target.sessionId!, expectedGeneration: target.generation!, directoryRef: target.directoryRef };
  const prepared = await prepareSftpTransferIntent({
    sourcePaneId: fence.source.paneId,
    targetPaneId: fence.target.paneId,
    sourceEndpointRevision: String(fence.sourceEndpointRevision),
    targetEndpointRevision: String(fence.targetEndpointRevision),
    source: sourceIntent,
    target: targetIntent,
    expectedBytes: entry.size,
    conflictPolicy,
  });
  const transfer = await enqueueSftpTransferIntent({ intentToken: prepared.intentToken });
  rememberTransferTarget(transfer.transferId, fence.target);
}

async function copyRecursiveDirectory(
  sourceParent: RecursiveDirectoryEndpoint,
  targetParent: RecursiveDirectoryEndpoint,
  rootEntry: RecursiveEntry,
  fence: ReturnType<typeof resolveSftpPaneDropFence> & {},
  budget: RecursiveCopyBudget,
  conflictPolicy: SftpConflictPolicy,
  existingTarget: RecursiveEntry | null = null,
  depth = 0,
) {
  if (depth >= maximumRecursiveCopyDepth) throw new Error("recursive copy depth exceeded");
  const source = await openRecursiveSourceDirectory(sourceParent, rootEntry);
  const target = await createRecursiveTargetDirectory(targetParent, rootEntry.displayName, existingTarget);
  try {
    const entries = await listRecursiveDirectory(source);
    const targetEntries = existingTarget ? await listRecursiveDirectory(target) : [];
    for (const entry of entries) {
      budget.entries += 1;
      if (budget.entries > maximumRecursiveCopyEntries) throw new Error("recursive copy entry limit exceeded");
      if (entry.kind === "file") {
        budget.bytes += entry.size ?? 0;
        if (budget.bytes > maximumRecursiveCopyBytes) throw new Error("recursive copy byte limit exceeded");
        const existingChild = targetEntries.find((candidate) => candidate.displayName === entry.displayName) ?? null;
        if (existingChild && (conflictPolicy !== "replaceSafely" || existingChild.kind !== "file")) {
          throw new Error("recursive target conflict");
        }
        await enqueueRecursiveFile(source, target, entry, fence, existingChild ? "replaceSafely" : "failIfExists");
      } else if (entry.kind === "directory") {
        const existingChild = targetEntries.find((candidate) => candidate.displayName === entry.displayName) ?? null;
        if (existingChild && (conflictPolicy !== "replaceSafely" || existingChild.kind !== "directory")) {
          throw new Error("recursive target conflict");
        }
        await copyRecursiveDirectory(source, target, entry, fence, budget, conflictPolicy, existingChild, depth + 1);
      } else {
        throw new Error("recursive copy contains unsupported entry");
      }
    }
  } finally {
    if (source.kind === "local" && source.revision) {
      await releaseSftpLocalDirectory({ directoryRef: source.directoryRef, expectedRevision: source.revision }).catch(() => undefined);
    }
    if (target.kind === "local" && target.revision) {
      await releaseSftpLocalDirectory({ directoryRef: target.directoryRef, expectedRevision: target.revision }).catch(() => undefined);
    }
  }
}

async function commitDrop(target: SftpPaneState) {
  if (paneInteractionPending(target.paneId)) return;
  const intent = dragIntent.value;
  dropPaneId.value = null;
  if (!intent) return;
  const fence = resolveSftpPaneDropFence(intent, paneStates, target.paneId);
  if (!fence || !fence.source.directoryRef || !fence.target.directoryRef) {
    target.error = t("sftp.dropUnsupported");
    return;
  }
  if (paneInteractionPending(fence.source.paneId) || paneInteractionPending(fence.target.paneId)) {
    return;
  }
  pendingPaneIds.add(fence.source.paneId);
  pendingPaneIds.add(fence.target.paneId);
  let directoryCopyCompleted = false;
  try {
    const existingTarget = matchingTargetEntry(fence.target, fence.entry.displayName);
    let conflictPolicy: SftpConflictPolicy = "failIfExists";
    if (existingTarget) {
      const decision = await requestOverwriteConfirmation(
        fence.entry.displayName,
        fence.entry.kind === "directory" ? "directory" : "file",
      );
      if (decision === "skip") return;
      const currentFence = resolveSftpPaneDropFence(intent, paneStates, target.paneId);
      if (!currentFence
        || currentFence.sourceEndpointRevision !== fence.sourceEndpointRevision
        || currentFence.targetEndpointRevision !== fence.targetEndpointRevision
        || currentFence.sourceDirectoryRevision !== fence.sourceDirectoryRevision
        || currentFence.targetDirectoryRevision !== fence.targetDirectoryRevision) {
        target.error = t("sftp.paneFenceChanged");
        return;
      }
      conflictPolicy = "replaceSafely";
    }
    if (fence.entry.kind === "directory") {
      const sourceEndpoint = recursiveEndpointForPane(fence.source);
      const targetEndpoint = recursiveEndpointForPane(fence.target);
      if (!sourceEndpoint || !targetEndpoint || (sourceEndpoint.kind === "local" && targetEndpoint.kind === "local")) {
        throw new Error("unsupported recursive copy endpoints");
      }
      const existingRecursiveTarget: RecursiveEntry | null = existingTarget ? {
        entryRef: existingTarget.entryRef,
        displayName: existingTarget.displayName,
        kind: existingTarget.kind,
        size: existingTarget.size,
        remotePathBytes: existingTarget.remotePathBytes ? [...existingTarget.remotePathBytes] : null,
      } : null;
      await copyRecursiveDirectory(sourceEndpoint, targetEndpoint, {
        entryRef: fence.entry.entryRef,
        displayName: fence.entry.displayName,
        kind: fence.entry.kind,
        size: fence.entry.size,
        remotePathBytes: fence.entry.remotePathBytes ? [...fence.entry.remotePathBytes] : null,
      }, fence, { entries: 1, bytes: 0 }, conflictPolicy, existingRecursiveTarget);
      await refreshSnapshot();
      directoryCopyCompleted = true;
    } else {
      if (fence.entry.size === null) throw new Error("file size unavailable");
      const source = fence.source.endpoint.kind === "local"
        ? { kind: "localDirectoryEntry" as const, directoryRef: fence.source.directoryRef, entryRef: fence.entry.entryRef }
        : { kind: "remoteFile" as const, sessionId: fence.source.endpoint.sessionId!, expectedGeneration: fence.source.endpoint.generation!, directoryRef: fence.source.directoryRef, entryRef: fence.entry.entryRef };
      const destination = fence.target.endpoint.kind === "local"
        ? { kind: "localDirectory" as const, directoryRef: fence.target.directoryRef }
        : { kind: "remoteDirectory" as const, sessionId: fence.target.endpoint.sessionId!, expectedGeneration: fence.target.endpoint.generation!, directoryRef: fence.target.directoryRef };
      const prepared = await prepareSftpTransferIntent({ sourcePaneId: fence.source.paneId, targetPaneId: fence.target.paneId, sourceEndpointRevision: String(fence.sourceEndpointRevision), targetEndpointRevision: String(fence.targetEndpointRevision), source, target: destination, expectedBytes: fence.entry.size, conflictPolicy });
      const currentFence = resolveSftpPaneDropFence(intent, paneStates, target.paneId);
      if (!currentFence || currentFence.sourceEndpointRevision !== fence.sourceEndpointRevision || currentFence.targetEndpointRevision !== fence.targetEndpointRevision || currentFence.sourceDirectoryRevision !== fence.sourceDirectoryRevision || currentFence.targetDirectoryRevision !== fence.targetDirectoryRevision) { target.error = t("sftp.paneFenceChanged"); return }
      const transfer = await enqueueSftpTransferIntent({ intentToken: prepared.intentToken });
      rememberTransferTarget(transfer.transferId, fence.target);
      await refreshSnapshot();
    }
  } catch {
    target.error = t("sftp.dropUnsupported");
    await refreshSnapshot().catch(() => undefined);
  } finally {
    pendingPaneIds.delete(fence.source.paneId);
    pendingPaneIds.delete(fence.target.paneId);
    dragIntent.value = null;
  }
  if (directoryCopyCompleted) {
    await (target.endpoint.kind === "local" ? loadLocalDirectory(target) : loadRemoteDirectory(target));
  }
}
async function cancelLegacyTransfer(transfer: SftpTransferSummary) {
  await cancelSftpTransfer({ transferId: transfer.transferId, expectedGeneration: transfer.generation, expectedStateRevision: transfer.stateRevision });
  await refreshSnapshot();
}
async function resumeLegacyTransfer(transfer: SftpTransferSummary) {
  const session = sessions.value.find((item) => item.sessionId === transfer.sessionId);
  if (!session || session.state !== "ready") return;
  await resumeSftpTransfer({ transferId: transfer.transferId, expectedGeneration: session.generation, expectedStateRevision: transfer.stateRevision });
  await refreshSnapshot();
}
function cleanupResidualDisplay(transfer: SftpTransferSummary | SftpTransferIntentSummary) {
  const residual = transfer.cleanupResidual;
  if (!residual) return "";
  return residual.kind === "remoteTemporaryTarget" ? residual.displayPath : residual.displayName;
}
async function retryIntentRemoteCleanup(transfer: SftpTransferIntentSummary) {
  const residual = transfer.cleanupResidual;
  if (residual?.kind !== "remoteTemporaryTarget") return;
  const session = sessions.value.find((item) => item.sessionId === residual.sessionId);
  const host = session ? hosts.value.find((item) => item.hostId === session.hostId) : null;
  if (!host) return;
  cleanupPendingTransferId.value = transfer.transferId;
  try {
    await retrySftpTransferIntentCleanup({
      transferId: transfer.transferId,
      expectedSourceFence: transfer.sourceFence,
      expectedTargetFence: transfer.targetFence,
      expectedStateRevision: transfer.stateRevision,
      expectedTargetHostStateVersion: host.stateVersion,
    });
    retainedForExit.value.delete(transfer.transferId);
    await refreshSnapshot();
  } catch { showOperationFailed(); } finally { cleanupPendingTransferId.value = null }
}
async function retryRemoteCleanup(transfer: SftpTransferSummary) {
  const session = sessions.value.find((item) => item.sessionId === transfer.sessionId);
  const host = session ? hosts.value.find((item) => item.hostId === session.hostId) : null;
  if (!host || transfer.cleanupResidual?.kind !== "remoteTemporaryTarget") return;
  cleanupPendingTransferId.value = transfer.transferId;
  try {
    await retrySftpRemoteCleanup({ transferId: transfer.transferId, expectedGeneration: transfer.generation, expectedStateRevision: transfer.stateRevision, expectedHostStateVersion: host.stateVersion });
    retainedForExit.value.delete(transfer.transferId);
    await refreshSnapshot();
  } catch { showOperationFailed(); } finally { cleanupPendingTransferId.value = null }
}
async function confirmRetainRemoteCleanup() {
  const transfer = cleanupRetainTarget.value;
  if (!transfer || transfer.cleanupResidual?.kind !== "remoteTemporaryTarget") return;
  cleanupPendingTransferId.value = transfer.transferId;
  try {
    if ("sourceFence" in transfer) {
      await retainSftpTransferIntentCleanupForExit({
        transferId: transfer.transferId,
        expectedSourceFence: transfer.sourceFence,
        expectedTargetFence: transfer.targetFence,
        expectedStateRevision: transfer.stateRevision,
        retainRemoteTemporaryFileConfirmed: true,
      });
    } else {
      await retainSftpRemoteCleanupForExit({ transferId: transfer.transferId, expectedGeneration: transfer.generation, expectedStateRevision: transfer.stateRevision, retainRemoteTemporaryFileConfirmed: true });
    }
    retainedForExit.value.add(transfer.transferId);
    cleanupRetainTarget.value = null;
  } catch { showOperationFailed(); } finally { cleanupPendingTransferId.value = null }
}
async function cancelIntentTransfer(transfer: SftpTransferIntentSummary) {
  await cancelSftpTransferIntent({ transferId: transfer.transferId, expectedSourceFence: transfer.sourceFence, expectedTargetFence: transfer.targetFence, expectedStateRevision: transfer.stateRevision });
  await refreshSnapshot();
}

let nativeFocusOperation = "";
const focusedTransferId = ref<string | null>(null);
watch([() => router.currentRoute.value.query.focusOperation, loading], async ([operation, isLoading]) => {
  if (isLoading || !sftpViewMounted || typeof operation !== "string" || operation === nativeFocusOperation || router.currentRoute.value.path !== "/sftp") return;
  nativeFocusOperation = operation;
  const query = { ...router.currentRoute.value.query };
  try {
    await refreshSnapshot();
    if (!sftpViewMounted || router.currentRoute.value.query.focusOperation !== operation) return;
    if (document.querySelector('[role="dialog"][aria-modal="true"]')) throw new Error("unavailable");
    if (query.focusTransfers === "true") {
      if (query.focusTransferId) {
        const target = takeNativeTransferNavigation(operation);
        if (!target || target.transferId !== query.focusTransferId || !matchesNativeTransferNavigation(target, intentTransfers.value, legacyTransfers.value)) throw new Error("unavailable");
        focusedTransferId.value = target.transferId;
      } else focusedTransferId.value = null;
      transfersOpen.value = true;
      await nextTick();
      transferActivityRoot.value?.querySelector<HTMLElement>('[data-native-focused="true"]')?.scrollIntoView({ block: "nearest" });
    } else {
      let pane = Object.values(paneStates).find((item) => item.endpoint.kind === "remote" && item.endpoint.sessionId === query.focusSessionId && item.endpoint.generation === query.focusGeneration);
      const session = sessions.value.find((item) => item.sessionId === query.focusSessionId && item.generation === query.focusGeneration);
      if (!session) throw new Error("unavailable");
      if (!pane) {
        pane = Object.values(paneStates).find((item) => item.endpoint.kind === "remote" && !item.endpoint.sessionId && !paneInteractionPending(item.paneId));
        if (!pane) {
          const paneId = crypto.randomUUID();
          sftpLayout.value = splitTerminalPane(sftpLayout.value, activeSftpPaneId.value, "horizontal", paneId, crypto.randomUUID());
          sftpLayout.value = setTerminalForPane(sftpLayout.value, paneId, "remote");
          paneStates[paneId] = createSftpPaneState(paneId, "remote", browserPreferences.browser);
          pane = paneStates[paneId]!;
        }
        attachSftpSessionToPane(pane, session);
      }
      if (paneInteractionPending(pane.paneId)) throw new Error("unavailable");
      activeSftpPaneId.value = pane.paneId;
      if (session.state === "ready" && !pane.directoryRef) await loadInitialRemoteDirectory(pane);
    }
  } catch { tips.show({ tone: "error", title: t("errors.tray.actionUnavailable") }); }
}, { immediate: true });

let removeToolListener: (() => void) | undefined;
let pageObserversActive = false;
let pageObserverEpoch = 0;
function startPageObservers() {
  if (pageObserversActive) return;
  pageObserversActive = true;
  const epoch = ++pageObserverEpoch;
  document.addEventListener("pointerdown", onDocumentPointerDown);
  document.addEventListener("keydown", onDocumentKeyDown);
  refreshTimer = window.setInterval(() => void refreshSnapshot().catch(() => undefined), 1000);
  if (canUseDesktopCore()) {
    void getCurrentWebview().onDragDropEvent((event) => handleNativeFileDrop(event.payload)).then((stop) => {
      if (pageObserversActive && pageObserverEpoch === epoch) stopNativeFileDrop = stop;
      else stop();
    }).catch(() => undefined);
  }
}
function stopPageObservers() {
  pageObserversActive = false;
  pageObserverEpoch += 1;
  document.removeEventListener("pointerdown", onDocumentPointerDown);
  document.removeEventListener("keydown", onDocumentKeyDown);
  stopNativeFileDrop?.();
  stopNativeFileDrop = null;
  if (refreshTimer !== null) window.clearInterval(refreshTimer);
  refreshTimer = null;
}
onMounted(async () => {
  if (canUseDesktopCore()) {
    try { removeToolListener = await onToolWindowChanged((kind) => { if (kind === "sftpFile") for (const pane of Object.values(paneStates)) refreshPane(pane); }); } catch { showOperationFailed(); }
  }
  sftpViewMounted = true;
  startPageObservers();
  try {
    const [hostList] = await Promise.all([
      listHosts(),
      refreshSnapshot(),
      initializeDefaultLocalPaneSafely(paneStates[localPaneId]!),
    ]);
    hosts.value = hostList;
    const pane = paneStates[remotePaneId];
    if (pane?.endpoint.kind === "remote" && !pane.endpoint.hostId && hostList[0] && !pendingSftpPluginNavigations.value.length) {
      const existingSession = unclaimedSftpSessionForHost(hostList[0].hostId, true);
      if (existingSession) {
        attachSftpSessionToPane(pane, existingSession);
        if (existingSession.state === "ready") await loadInitialRemoteDirectory(pane);
      }
      else replaceSftpPaneEndpoint(pane, {
        kind: "remote",
        hostId: hostList[0].hostId,
        sessionId: null,
        generation: null,
      }, "/", [47]);
    }
  } catch { showOperationFailed(); } finally { navigationReady = true; loading.value = false }
});
onActivated(() => {
  if (!sftpViewMounted) return;
  startPageObservers();
  if (navigationReady) {
    void refreshSnapshot().catch(() => undefined);
    void consumePluginNavigation();
  }
});
onDeactivated(() => {
  stopPageObservers();
  transfersOpen.value = false;
  contextMenu.value = null;
  pointerDrag.value = null;
  dragIntent.value = null;
  dropPaneId.value = null;
  settleOverwriteConfirmation("skip");
  closeFilePreview();
  mutationDialog.value = null;
  mutationTargetPaneId.value = null;
  fileUtilityDialog.value = null;
  fileUtilityTargetPaneId.value = null;
  permissionsTarget.value = null;
  cleanupRetainTarget.value = null;
  closeRemotePaneTargetId.value = null;
});
onBeforeUnmount(() => {
  removeToolListener?.();
  sftpViewMounted = false;
  navigationReady = false;
  stopPageObservers();
  closeFilePreview();
  settleOverwriteConfirmation("skip");
  for (const paneId of Object.keys(paneStates)) {
    void cancelRemotePaneCursor(paneStates[paneId]);
    void releaseLocalPane(paneId);
  }
});
</script>

<template>
  <main class="sftp-view">
    <header class="sftp-view__topbar">
      <div class="sftp-view__title">
        <NvxIcon
          :icon="FolderOpen"
          :size="20"
        /><h1>{{ t("sftp.title") }}</h1>
      </div>
      <div
        ref="transferActivityRoot"
        class="sftp-view__transfer-activity"
      >
        <NvxPluginExtensionTarget
          target-id="sftp.toolbar"
          instance-key="global"
        />
        <NvxButton
          class="sftp-view__transfer-toggle"
          data-sftp-transfer-toggle
          size="sm"
          variant="ghost"
          :aria-label="activeTransferProgress === null ? t('sftp.transferCount', { count: allTransferCount }) : t('sftp.transferButtonProgress', { count: allTransferCount, percent: activeTransferProgress })"
          :aria-controls="transfersOpen ? 'sftp-transfer-activity-panel' : undefined"
          :aria-expanded="transfersOpen"
          @click="toggleTransferActivity"
        >
          <NvxIcon
            :icon="Activity"
            :size="16"
          />
          {{ t("sftp.transferCount", { count: allTransferCount }) }}
          <span
            v-if="activeTransferProgress !== null"
            class="sftp-view__transfer-toggle-percent"
          >{{ activeTransferProgress }}%</span>
          <span
            v-if="activeTransferProgress !== null"
            class="sftp-view__transfer-toggle-track"
            aria-hidden="true"
          ><span :style="{ width: `${activeTransferProgress}%` }" /></span>
        </NvxButton>

        <section
          v-if="transfersOpen"
          id="sftp-transfer-activity-panel"
          class="sftp-view__transfers"
          :class="{ 'is-empty': allTransferCount === 0 }"
          :aria-label="t('sftp.transferActivity')"
        >
          <header>
            <div>
              <h2>{{ t("sftp.transferActivity") }}</h2>
              <span>{{ t("sftp.transferCount", { count: allTransferCount }) }}</span>
            </div>
            <NvxIconButton
              :label="t('sftp.closeTransferActivity')"
              size="sm"
              @click="closeTransferActivity(true)"
            >
              <NvxIcon
                :icon="X"
                :size="16"
              />
            </NvxIconButton>
          </header>
          <p
            v-if="allTransferCount === 0"
            class="sftp-view__transfer-empty"
          >
            {{ t("sftp.noTransfers") }}
          </p>
          <article
            v-for="transfer in intentTransfers"
            :key="transfer.transferId"
            class="sftp-view__transfer"
            :data-native-focused="focusedTransferId === transfer.transferId"
            :class="{ 'sftp-view__transfer--focused': focusedTransferId === transfer.transferId }"
          >
            <div class="sftp-view__transfer-title">
              <strong>{{ transfer.sourceDisplayName }} → {{ transfer.targetDisplayName }}</strong><NvxStatusLabel :tone="stateTone(transfer.state)">
                {{ t(`sftp.transferStates.${transfer.state}`) }}
              </NvxStatusLabel>
            </div>
            <NvxProgress
              :label="t('sftp.progress')"
              :value="progress(transfer)"
              size="sm"
            >
              <template #value>
                {{ formatTransferBytes(transfer.transferredBytes) }} / {{ formatTransferBytes(transfer.expectedBytes) }}
              </template>
            </NvxProgress>
            <div class="sftp-view__transfer-meta">
              <span>{{ t(`sftp.directions.${transfer.direction}`) }}</span>
              <span v-if="transfer.state === 'transferring'">{{ t('sftp.speed') }} {{ transfer.bytesPerSecond === null ? t('sftp.pendingFact') : t('sftp.bytesPerSecond', { count: formatTransferBytes(transfer.bytesPerSecond) }) }}</span>
              <span v-if="transfer.state === 'transferring' && transfer.remainingSeconds !== null">{{ t('sftp.remaining') }} {{ t('sftp.seconds', { count: transfer.remainingSeconds }) }}</span>
              <span v-if="transfer.commitOutcome !== 'notCommitted'">{{ t(`sftp.commitOutcomes.${transfer.commitOutcome}`) }}</span>
            </div>
            <NvxInlineNotice
              v-if="transfer.failureCode"
              tone="error"
              :title="t(`sftp.failures.${transfer.failureCode}`)"
            >
              <span v-if="transfer.cleanupResidual">{{ t("sftp.cleanupResidual", { path: cleanupResidualDisplay(transfer) }) }}</span>
              <span v-if="retainedForExit.has(transfer.transferId)">{{ t("sftp.cleanupRetainedForExit") }}</span>
            </NvxInlineNotice>
            <div class="sftp-view__transfer-actions">
              <NvxPluginExtensionTarget
                target-id="sftp.transfer.actions"
                :instance-key="transfer.transferId"
              />
              <NvxButton
                v-if="!['committing', 'completed', 'cancelled', 'failed'].includes(transfer.state)"
                size="sm"
                variant="secondary"
                @click="cancelIntentTransfer(transfer)"
              >
                {{ t("sftp.cancel") }}
              </NvxButton>
              <NvxButton
                v-if="transfer.cleanupResidual?.kind === 'remoteTemporaryTarget' && transfer.commitOutcome !== 'uncertain'"
                size="sm"
                variant="secondary"
                :loading="cleanupPendingTransferId === transfer.transferId"
                @click="retryIntentRemoteCleanup(transfer)"
              >
                <NvxIcon
                  :icon="RotateCcw"
                  :size="16"
                />{{ t("sftp.retryRemoteCleanup") }}
              </NvxButton>
              <NvxButton
                v-if="transfer.cleanupResidual?.kind === 'remoteTemporaryTarget'"
                size="sm"
                variant="danger"
                @click="cleanupRetainTarget = transfer"
              >
                <NvxIcon
                  :icon="ShieldCheck"
                  :size="16"
                />{{ t("sftp.retainRemoteForExit") }}
              </NvxButton>
            </div>
          </article>
          <article
            v-for="transfer in displayedLegacyTransfers"
            :key="transfer.transferId"
            class="sftp-view__transfer"
            :data-native-focused="focusedTransferId === transfer.transferId"
            :class="{ 'sftp-view__transfer--focused': focusedTransferId === transfer.transferId }"
          >
            <div class="sftp-view__transfer-title">
              <strong>{{ t(`sftp.directions.${transfer.direction}`) }}</strong><NvxStatusLabel :tone="stateTone(transfer.state)">
                {{ t(`sftp.transferStates.${transfer.state}`) }}
              </NvxStatusLabel>
            </div>
            <NvxProgress
              :label="t('sftp.progress')"
              :value="progress(transfer)"
              size="sm"
            >
              <template #value>
                {{ formatTransferBytes(transfer.transferredBytes) }} / {{ formatTransferBytes(transfer.expectedBytes) }}
              </template>
            </NvxProgress>
            <div class="sftp-view__transfer-meta">
              <span v-if="transfer.state === 'transferring'">{{ t('sftp.speed') }} {{ transfer.bytesPerSecond === null ? t('sftp.pendingFact') : t('sftp.bytesPerSecond', { count: formatTransferBytes(transfer.bytesPerSecond) }) }}</span>
              <span v-if="transfer.state === 'transferring' && transfer.remainingSeconds !== null">{{ t('sftp.remaining') }} {{ t('sftp.seconds', { count: transfer.remainingSeconds }) }}</span>
            </div>
            <NvxInlineNotice
              v-if="transfer.failureCode"
              tone="error"
              :title="t(`sftp.failures.${transfer.failureCode}`)"
            >
              <span v-if="transfer.commitOutcome === 'uncertain'">{{ t('sftp.commitOutcomes.uncertain') }}</span>
              <span v-if="transfer.cleanupResidual">{{ t("sftp.cleanupResidual", { path: cleanupResidualDisplay(transfer) }) }}</span>
              <span v-if="retainedForExit.has(transfer.transferId)">{{ t("sftp.cleanupRetainedForExit") }}</span>
            </NvxInlineNotice>
            <NvxInlineNotice
              v-if="transfer.state === 'pausedByDisconnect'"
              tone="warning"
              :title="t('sftp.recoveryCheckHint')"
            />
            <div class="sftp-view__transfer-actions">
              <NvxPluginExtensionTarget
                target-id="sftp.transfer.actions"
                :instance-key="transfer.transferId"
              />
              <NvxButton
                v-if="!['committing', 'completed', 'cancelled', 'failed'].includes(transfer.state)"
                size="sm"
                variant="secondary"
                @click="cancelLegacyTransfer(transfer)"
              >
                {{ t("sftp.cancel") }}
              </NvxButton>
              <NvxButton
                v-if="['failed', 'pausedByDisconnect'].includes(transfer.state) && transfer.commitOutcome !== 'uncertain' && !transfer.cleanupResidual"
                size="sm"
                variant="secondary"
                @click="resumeLegacyTransfer(transfer)"
              >
                {{ t("sftp.inspectRecovery") }}
              </NvxButton>
              <NvxButton
                v-if="transfer.cleanupResidual?.kind === 'remoteTemporaryTarget' && transfer.commitOutcome !== 'uncertain'"
                size="sm"
                variant="secondary"
                :loading="cleanupPendingTransferId === transfer.transferId"
                @click="retryRemoteCleanup(transfer)"
              >
                <NvxIcon
                  :icon="RotateCcw"
                  :size="16"
                />{{ t("sftp.retryRemoteCleanup") }}
              </NvxButton>
              <NvxButton
                v-if="transfer.cleanupResidual?.kind === 'remoteTemporaryTarget'"
                size="sm"
                variant="danger"
                @click="cleanupRetainTarget = transfer"
              >
                <NvxIcon
                  :icon="ShieldCheck"
                  :size="16"
                />{{ t("sftp.retainRemoteForExit") }}
              </NvxButton>
            </div>
          </article>
        </section>
      </div>
    </header>
    <section class="sftp-view__browser">
      <NvxTerminalSplitTree
        class="sftp-view__workspace"
        :node="sftpLayout"
        :active-pane-id="activeSftpPaneId"
        :separator-label="t('sftp.resizePanes')"
        :minimum-pane-width="420"
        :minimum-pane-height="240"
        @activate="activeSftpPaneId = $event"
        @resize="resizeSftpSplit"
      >
        <template #pane="{ pane, canSplitHorizontal, canSplitVertical }">
          <section
            v-if="paneStates[pane.paneId]"
            class="sftp-view__pane"
            :class="{ 'is-drop-target': dropPaneId === pane.paneId }"
          >
            <header class="sftp-view__pane-header">
              <div class="sftp-view__pane-identity">
                <NvxIcon
                  :icon="sftpPaneKind(pane) === 'local' ? HardDrive : Server"
                  :size="20"
                /><strong>{{ sftpPaneKind(pane) === "local" ? t("sftp.localPane") : paneRemoteLabel(paneState(pane.paneId)) }}</strong>
              </div>
              <div
                v-if="sftpPaneKind(pane) === 'remote'"
                class="sftp-view__pane-endpoint"
              >
                <NvxSelect
                  v-if="!sessionForPane(paneState(pane.paneId))?.parentSshSession || paneHostId(paneState(pane.paneId))"
                  :model-value="paneHostId(paneState(pane.paneId))"
                  class="sftp-view__host-control"
                  :options="hostOptions"
                  :disabled="paneHasSession(paneState(pane.paneId)) || paneInteractionPending(pane.paneId)"
                  :aria-label="t('sftp.paneHost')"
                  @update:model-value="setPaneHostId(paneState(pane.paneId), $event)"
                />
                <span
                  v-else
                  class="sftp-view__parent-label"
                >{{ paneRemoteLabel(paneState(pane.paneId)) }}</span>
                <NvxStatusLabel
                  v-if="sessionForPane(paneState(pane.paneId))"
                  :tone="stateTone(sessionForPane(paneState(pane.paneId))!.state)"
                  :title="paneSessionFailure(paneState(pane.paneId)) ? sessionFailureLabel(paneSessionFailure(paneState(pane.paneId))!.code) : undefined"
                >
                  {{ t(`sftp.states.${sessionForPane(paneState(pane.paneId))!.state}`) }}
                </NvxStatusLabel>
                <NvxButton
                  v-if="!sessionForPane(paneState(pane.paneId)) || ['failed', 'closed'].includes(sessionForPane(paneState(pane.paneId))!.state)"
                  size="sm"
                  :loading="paneInteractionPending(pane.paneId)"
                  :disabled="loading || !paneHostId(paneState(pane.paneId)) || paneInteractionPending(pane.paneId)"
                  @click="recoverRemotePane(paneState(pane.paneId))"
                >
                  <NvxIcon
                    :icon="PlugZap"
                    :size="16"
                  />{{ paneConnectionLabel(paneState(pane.paneId)) }}
                </NvxButton>
                <NvxButton
                  v-else
                  size="sm"
                  variant="ghost"
                  :loading="paneInteractionPending(pane.paneId)"
                  :disabled="paneInteractionPending(pane.paneId)"
                  @click="disconnectRemotePane(paneState(pane.paneId))"
                >
                  <NvxIcon
                    :icon="Square"
                    :size="16"
                  />{{ t("sftp.disconnect") }}
                </NvxButton>
              </div>
              <div class="sftp-view__pane-controls">
                <NvxSftpPaneActionsMenu
                  :pane-id="pane.paneId"
                  class="sftp-view__pane-actions-menu"
                  :kind="sftpPaneKind(pane)"
                  :ready="paneReady(paneState(pane.paneId))"
                  :has-selection="paneState(pane.paneId).selectedEntryKeys.length > 0"
                  :single-selection="paneState(pane.paneId).selectedEntryKeys.length === 1"
                  :pending="paneInteractionPending(pane.paneId)"
                  :show-hidden="paneState(pane.paneId).showHidden"
                  :folders-first="paneState(pane.paneId).foldersFirst"
                  :can-split-horizontal="canSplitHorizontal"
                  :can-split-vertical="canSplitVertical"
                  :can-close="countTerminalPanes(sftpLayout) > 1"
                  @choose-folder="chooseLocalFolder(paneState(pane.paneId))"
                  @upload="uploadFilesFromPicker(paneState(pane.paneId))"
                  @download="downloadSelectedFromPicker(paneState(pane.paneId))"
                  @mkdir="activeSftpPaneId = pane.paneId; openMutation('mkdir')"
                  @rename="activeSftpPaneId = pane.paneId; openMutation('rename')"
                  @delete="activeSftpPaneId = pane.paneId; openMutation('delete')"
                  @toggle-show-hidden="togglePaneShowHidden(paneState(pane.paneId))"
                  @toggle-folders-first="togglePaneFoldersFirst(paneState(pane.paneId))"
                  @split="splitSftpPane(pane.paneId, $event)"
                  @add-remote="splitSftpPane(pane.paneId, canSplitHorizontal ? 'horizontal' : 'vertical', 'remote')"
                  @close="requestCloseSftpPane(pane.paneId)"
                />
              </div>
            </header>
            <div class="sftp-view__commandbar">
              <NvxIconButton
                :label="t('sftp.up')"
                size="sm"
                :disabled="paneInteractionPending(pane.paneId) || !paneReady(paneState(pane.paneId)) || (sftpPaneKind(pane) === 'remote' ? paneState(pane.paneId).directory === '/' : !(localTrailByPane.get(pane.paneId)?.length))"
                @click="goUp(paneState(pane.paneId))"
              >
                <NvxIcon
                  :icon="ArrowUp"
                  :size="16"
                />
              </NvxIconButton>
              <span class="sftp-view__command-separator" />
              <div class="sftp-view__command-path">
                <NvxIcon
                  :icon="FolderOpen"
                  :size="16"
                />
                <NvxInput
                  :model-value="pathDraftByPane[pane.paneId] ?? paneState(pane.paneId).directory"
                  :aria-label="sftpPaneKind(pane) === 'local' ? t('sftp.localPath') : t('sftp.remotePath')"
                  :disabled="paneInteractionPending(pane.paneId) || (sftpPaneKind(pane) === 'remote' && !paneReady(paneState(pane.paneId)))"
                  @focus="pathSuggestionsOpenByPane[pane.paneId] = true"
                  @update:model-value="updatePathDraft(paneState(pane.paneId), $event)"
                  @keydown="onPathKeydown($event, paneState(pane.paneId))"
                />
                <ul
                  v-if="pathSuggestionsOpenByPane[pane.paneId] && pathSuggestions(paneState(pane.paneId)).length"
                  class="sftp-view__path-suggestions"
                  role="listbox"
                  :aria-label="t('sftp.localPath')"
                >
                  <li
                    v-for="(suggestion, index) in pathSuggestions(paneState(pane.paneId))"
                    :key="suggestion"
                    role="option"
                    :aria-selected="pathSuggestionIndexByPane[pane.paneId] === index"
                  >
                    <button
                      type="button"
                      :class="{ 'is-active': pathSuggestionIndexByPane[pane.paneId] === index }"
                      @pointerdown.prevent
                      @click="choosePathSuggestion(paneState(pane.paneId), suggestion)"
                    >
                      {{ suggestion }}
                    </button>
                  </li>
                </ul>
              </div>
              <NvxIconButton
                :label="t('sftp.refresh')"
                size="sm"
                :disabled="paneInteractionPending(pane.paneId) || !paneReady(paneState(pane.paneId)) || paneState(pane.paneId).loading"
                @click="refreshPane(paneState(pane.paneId))"
              >
                <NvxIcon
                  :icon="RefreshCw"
                  :size="16"
                />
              </NvxIconButton>
              <span class="sftp-view__command-separator" />
              <div class="sftp-view__command-search">
                <NvxIconButton
                  :class="{ 'is-active': searchOpenByPane[pane.paneId] || paneState(pane.paneId).search }"
                  :label="t('sftp.search')"
                  size="sm"
                  :aria-expanded="Boolean(searchOpenByPane[pane.paneId])"
                  @click="searchOpenByPane[pane.paneId] = !searchOpenByPane[pane.paneId]"
                >
                  <NvxIcon
                    :icon="Search"
                    :size="16"
                  />
                </NvxIconButton>
                <NvxInput
                  v-if="searchOpenByPane[pane.paneId]"
                  :model-value="paneState(pane.paneId).search"
                  class="sftp-view__search-popover"
                  :placeholder="t('sftp.searchPlaceholder')"
                  :aria-label="t('sftp.search')"
                  @update:model-value="setPaneSearch(paneState(pane.paneId), $event)"
                  @keydown.esc="searchOpenByPane[pane.paneId] = false"
                  @keydown.enter="searchOpenByPane[pane.paneId] = false"
                />
              </div>
              <span class="sftp-view__command-sort">
                <NvxSelect
                  v-model="paneState(pane.paneId).sort"
                  :options="sortOptions"
                  :aria-label="t('sftp.sort')"
                  :popup-min-width="160"
                />
                <NvxIcon
                  :icon="ListFilter"
                  :size="16"
                />
              </span>
            </div>
            <div
              class="sftp-view__columns"
              aria-hidden="true"
            >
              <span>{{ t("sftp.nameColumn") }}<small
                v-if="paneState(pane.paneId).selectedEntryKeys.length"
                class="sftp-view__selection-count"
              > · {{ t("sftp.selectedItems", { count: paneState(pane.paneId).selectedEntryKeys.length }) }}</small></span><span>{{ t("sftp.sizeColumn") }}</span><span>{{ t("sftp.modifiedColumn") }}</span>
            </div>
            <div
              class="sftp-view__entries"
              role="listbox"
              aria-multiselectable="true"
              :aria-label="t('sftp.remoteFiles')"
              :title="t('sftp.multiSelectHint')"
              @contextmenu="openSftpContextMenu($event, paneState(pane.paneId))"
            >
              <button
                v-for="entry in paneEntries(pane.paneId)"
                :key="entry.key"
                type="button"
                role="option"
                :disabled="paneInteractionPending(pane.paneId)"
                :title="['file', 'directory'].includes(entry.kind) ? t('sftp.fileDragHint') : t('sftp.symlinkDragDisabled')"
                :aria-selected="paneState(pane.paneId).selectedEntryKeys.includes(entry.key)"
                :class="{ 'is-selected': paneState(pane.paneId).selectedEntryKeys.includes(entry.key) }"
                @click="selectEntryFromClick($event, paneState(pane.paneId), entry)"
                @dblclick="activateEntry(paneState(pane.paneId), entry)"
                @contextmenu="openSftpContextMenu($event, paneState(pane.paneId), entry)"
                @pointerdown="beginPointerDrag($event, paneState(pane.paneId), entry)"
                @pointermove="updatePointerDrag"
                @pointerup="finishPointerDrag"
                @pointercancel="cancelPointerDrag"
              >
                <span class="sftp-view__entry-name"><NvxIcon
                  :icon="sftpEntryIcon(entry)"
                  :size="16"
                /><span>{{ entry.displayName }}</span></span>
                <small>{{ entry.kind === "file" && entry.size !== null ? t("sftp.bytes", { count: entry.size }) : t(`sftp.entryKinds.${entry.kind}`) }}</small><small>{{ formatModified(entry.modifiedAtUnixMs) }}</small>
              </button>
              <div
                v-if="paneState(pane.paneId).error"
                class="sftp-view__pane-error"
              >
                <p>{{ paneState(pane.paneId).error }}</p>
                <NvxButton
                  v-if="sftpPaneKind(pane) === 'remote' && sessionForPane(paneState(pane.paneId))?.state === 'ready'"
                  size="sm"
                  variant="secondary"
                  :disabled="paneInteractionPending(pane.paneId)"
                  @click="loadRemoteDirectory(paneState(pane.paneId), [47])"
                >
                  {{ t("sftp.reopenRoot") }}
                </NvxButton>
              </div>
              <p v-else-if="!paneReady(paneState(pane.paneId))">
                {{ sftpPaneKind(pane) === "local" ? t("sftp.localPickerTitle") : t("sftp.paneNotConnected") }}
              </p>
              <p v-else-if="!paneState(pane.paneId).loading && !paneState(pane.paneId).entries.length">
                {{ t("sftp.emptyDirectory") }}
              </p>
              <NvxButton
                v-if="nextCursorByPane[pane.paneId]"
                size="sm"
                variant="ghost"
                :loading="paneState(pane.paneId).loading"
                @click="loadMore(paneState(pane.paneId))"
              >
                {{ t("sftp.loadMore") }}
              </NvxButton>
            </div>
          </section>
        </template>
      </NvxTerminalSplitTree>
    </section>

    <div
      v-if="contextMenu"
      ref="contextMenuRoot"
      class="sftp-view__context-menu"
      role="menu"
      :aria-label="t('sftp.fileContextMenu')"
      :style="{ left: `${contextMenu.left}px`, top: `${contextMenu.top}px` }"
    >
      <button
        type="button"
        role="menuitem"
        @click="runSftpContextAction('refresh')"
      >
        <NvxIcon
          :icon="RefreshCw"
          :size="16"
        />
        {{ t("sftp.refresh") }}
      </button>
      <button
        type="button"
        role="menuitem"
        @click="runSftpContextAction('mkdir')"
      >
        <NvxIcon
          :icon="FolderPlus"
          :size="16"
        />
        {{ t("sftp.newFolder") }}
      </button>
      <button
        type="button"
        role="menuitem"
        @click="runSftpContextAction('touch')"
      >
        <NvxIcon
          :icon="FilePlus2"
          :size="16"
        />
        {{ t("sftp.newEmptyFile") }}
      </button>
      <button
        type="button"
        role="menuitem"
        @click="runSftpContextAction('downloadUrl')"
      >
        <NvxIcon
          :icon="Link2"
          :size="16"
        />
        {{ t("sftp.downloadFromUrl") }}
      </button>
      <template v-if="paneStates[contextMenu.paneId]?.selectedEntryKeys.length">
        <span role="separator" />
        <button
          v-if="contextSelectedEntries.every((entry) => ['file', 'directory'].includes(entry.kind))"
          type="button"
          role="menuitem"
          @click="runSftpContextAction('compress')"
        >
          <NvxIcon
            :icon="Archive"
            :size="16"
          />
          {{ t("sftp.compressZip") }}
        </button>
        <button
          v-if="contextSelectedEntries.length === 1 && contextSelectedEntry?.kind === 'file' && contextSelectedEntry.displayName.toLowerCase().endsWith('.zip')"
          type="button"
          role="menuitem"
          @click="runSftpContextAction('extract')"
        >
          <NvxIcon
            :icon="FileArchive"
            :size="16"
          />
          {{ t("sftp.extractZip") }}
        </button>
        <button
          v-if="contextSelectedEntries.length === 1 && contextSelectedEntry?.kind === 'file'"
          type="button"
          role="menuitem"
          :disabled="!previewKindForEntry(contextSelectedEntry)"
          :title="previewKindForEntry(contextSelectedEntry) ? undefined : t('sftp.previewUnavailable')"
          @click="runSftpContextAction('preview')"
        >
          <NvxIcon
            :icon="Eye"
            :size="16"
          />
          {{ t("sftp.preview") }}
        </button>
        <button
          v-if="contextSelectedEntries.length === 1 && previewKindForEntry(contextSelectedEntry) === 'text'"
          type="button"
          role="menuitem"
          @click="runSftpContextAction('tail')"
        >
          <NvxIcon
            :icon="Radio"
            :size="16"
          />
          {{ t("sftp.tailOpen") }}
        </button>
        <button
          v-if="contextSelectedEntries.length === 1 && contextSelectedEntry?.kind === 'file'"
          type="button"
          role="menuitem"
          @click="runSftpContextAction('download')"
        >
          <NvxIcon
            :icon="Download"
            :size="16"
          />
          {{ t("sftp.downloadFile") }}
        </button>
        <span
          v-if="contextSelectedEntries.length === 1 && contextSelectedEntry?.kind === 'file'"
          role="separator"
        />
        <button
          v-if="contextSelectedEntries.length === 1"
          type="button"
          role="menuitem"
          @click="runSftpContextAction('rename')"
        >
          <NvxIcon
            :icon="Pencil"
            :size="16"
          />
          {{ t("sftp.rename") }}
        </button>
        <button
          v-if="contextSelectedEntries.length === 1 && contextSelectedEntry && ['file', 'directory'].includes(contextSelectedEntry.kind)"
          type="button"
          role="menuitem"
          :disabled="contextSelectedEntry.permissionBits == null"
          :title="contextSelectedEntry.permissionBits == null ? t('sftp.permissions.unavailable') : undefined"
          @click="runSftpContextAction('permissions')"
        >
          <NvxIcon
            :icon="ShieldCheck"
            :size="16"
          />
          {{ t("sftp.permissions.menu") }}
        </button>
        <button
          class="sftp-view__context-menu-danger"
          type="button"
          role="menuitem"
          @click="runSftpContextAction('delete')"
        >
          <NvxIcon
            :icon="Trash2"
            :size="16"
          />
          {{ t("sftp.delete") }}
        </button>
      </template>
    </div>

    <NvxDialog
      plugin-protected
      :model-value="previewOpen"
      :title="previewName"
      :description="t(previewMode === 'tail' ? 'sftp.tailDescription' : 'sftp.previewDescription')"
      :close-label="t('sftp.closePreview')"
      size="lg"
      @update:model-value="(value) => { if (!value) closeFilePreview() }"
    >
      <div
        class="sftp-view__preview"
        :class="{ 'is-text': previewContent?.kind === 'text' }"
      >
        <p v-if="previewLoading">
          {{ t("sftp.previewLoading") }}
        </p>
        <NvxInlineNotice
          v-else-if="previewError"
          tone="error"
          :title="t('sftp.previewFailed')"
        >
          {{ t("sftp.previewFailedDescription") }}
        </NvxInlineNotice>
        <template v-else-if="previewContent?.kind === 'text'">
          <div class="sftp-view__preview-toolbar">
            <NvxStatusLabel
              v-if="previewMode === 'tail' && previewTailActive"
              tone="success"
            >
              {{ t("sftp.tailLive") }}
            </NvxStatusLabel>
            <span v-else>{{ previewMode === "tail" ? t("sftp.tailPaused") : previewTextEditable ? t("sftp.editorEditable") : t("sftp.editorReadOnly") }}</span>
            <NvxButton
              v-if="previewMode === 'tail'"
              size="sm"
              :variant="previewTailActive ? 'secondary' : 'ghost'"
              @click="togglePreviewTail"
            >
              <NvxIcon
                :icon="previewTailActive ? Pause : Radio"
                :size="16"
              />
              {{ previewTailActive ? t("sftp.tailStop") : t("sftp.tailStart") }}
            </NvxButton>
          </div>
          <NvxInlineNotice
            v-if="previewTailError"
            tone="error"
            :title="t('sftp.tailFailed')"
          >
            {{ t("sftp.tailFailedDescription") }}
          </NvxInlineNotice>
          <NvxInlineNotice
            v-else-if="previewSaveTooLarge"
            tone="warning"
            :title="t('sftp.editorTooLarge')"
          />
          <NvxInlineNotice
            v-else-if="previewSaveError"
            tone="error"
            :title="t('sftp.saveFailed')"
          >
            {{ t("sftp.saveFailedDescription") }}
          </NvxInlineNotice>
          <NvxInlineNotice
            v-else-if="previewTailReset || previewTailTrimmed"
            tone="info"
            :title="previewTailReset ? t('sftp.tailReset') : t('sftp.tailTrimmed')"
          />
          <NvxInlineNotice
            v-else-if="previewMode === 'preview' && previewContent.truncated"
            tone="info"
            :title="t('sftp.previewTruncated')"
          />
          <NvxCodeEditor
            v-model="previewText"
            class="sftp-view__code-editor"
            :filename="previewName"
            :readonly="previewEditorReadonly"
            :follow-end="previewTailActive"
            :label="t('sftp.editorLabel', { name: previewName })"
          />
        </template>
        <img
          v-else-if="previewContent?.kind === 'image' && previewImageUrl"
          :src="previewImageUrl"
          :alt="previewName"
        >
      </div>
      <template #actions>
        <NvxButton
          variant="secondary"
          @click="closeFilePreview"
        >
          {{ t("sftp.closePreview") }}
        </NvxButton>
        <NvxButton
          v-if="previewContent?.kind === 'text' && previewMode === 'preview'"
          :loading="previewSaving"
          :disabled="previewEditorReadonly || !previewDirty || previewSaveTooLarge"
          @click="savePreviewText"
        >
          <NvxIcon
            :icon="Save"
            :size="16"
          />
          {{ t("sftp.saveFile") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      plugin-protected
      :model-value="closeRemotePaneTargetId !== null"
      :title="t('sftp.closeRemotePaneDialog.title')"
      :description="t('sftp.closeRemotePaneDialog.description')"
      :close-label="t('sftp.closeRemotePaneDialog.close')"
      :dismissible="!closeRemotePanePending"
      @update:model-value="(value) => { if (!value && !closeRemotePanePending) closeRemotePaneTargetId = null }"
    >
      <NvxInlineNotice
        tone="warning"
        :title="t('sftp.closeRemotePaneDialog.warning')"
      />
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="closeRemotePanePending"
          @click="closeRemotePaneTargetId = null"
        >
          {{ t("sftp.closeRemotePaneDialog.cancel") }}
        </NvxButton><NvxButton
          variant="danger"
          :loading="closeRemotePanePending"
          @click="disconnectAndCloseRemotePane"
        >
          {{ t("sftp.closeRemotePaneDialog.confirm") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      plugin-protected
      :model-value="overwritePrompt !== null"
      :title="t('sftp.overwriteDialog.title')"
      :description="overwritePrompt ? t(`sftp.overwriteDialog.${overwritePrompt.kind}Description`, { name: overwritePrompt.displayName }) : ''"
      :close-label="t('sftp.overwriteDialog.close')"
      @update:model-value="(value) => { if (!value) settleOverwriteConfirmation('skip') }"
    >
      <NvxInlineNotice
        tone="warning"
        :title="t(overwritePrompt?.allowReplaceAll ? 'sftp.overwriteDialog.warningBatch' : 'sftp.overwriteDialog.warning')"
      >
        {{ overwritePrompt?.displayName }}
      </NvxInlineNotice>
      <template #actions>
        <NvxButton
          variant="secondary"
          @click="settleOverwriteConfirmation('skip')"
        >
          {{ t("sftp.overwriteDialog.cancel") }}
        </NvxButton><NvxButton
          variant="danger"
          @click="settleOverwriteConfirmation('replaceOnce')"
        >
          {{ t("sftp.overwriteDialog.confirm") }}
        </NvxButton><NvxButton
          v-if="overwritePrompt?.allowReplaceAll"
          variant="danger"
          @click="settleOverwriteConfirmation('replaceAll')"
        >
          {{ t("sftp.overwriteDialog.confirmAll") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      plugin-protected
      :model-value="mutationDialog !== null"
      :title="mutationDialog ? t(`sftp.mutations.${mutationDialog}.title`) : ''"
      :description="mutationDialog ? t(`sftp.mutations.${mutationDialog}.description`) : ''"
      :close-label="t('sftp.closeMutation')"
      :dismissible="!mutationPanePending"
      @update:model-value="(value) => { if (!value && !mutationPanePending) { mutationDialog = null; mutationTargetPaneId = null } }"
    >
      <NvxField
        v-if="mutationDialog !== 'delete'"
        for-id="sftp-mutation-name"
        :label="t('sftp.name')"
      >
        <NvxInput
          id="sftp-mutation-name"
          v-model="mutationName"
          :invalid="!mutationNameValid && mutationName.length > 0"
        />
      </NvxField>
      <NvxInlineNotice
        v-else
        tone="warning"
        :title="t('sftp.deleteIrreversible')"
      >
        {{ t('sftp.deleteSelectionSummary', {
          count: activeSelectedEntries.length,
          names: activeSelectedEntries.slice(0, 3).map((entry) => entry.displayName).join(', '),
        }) }}
      </NvxInlineNotice>
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="mutationPanePending"
          @click="mutationDialog = null; mutationTargetPaneId = null"
        >
          {{ t("sftp.cancelMutation") }}
        </NvxButton><NvxButton
          :variant="mutationDialog === 'delete' ? 'danger' : 'primary'"
          :loading="mutationPanePending"
          :disabled="mutationPanePending || (mutationDialog !== 'delete' && !mutationNameValid)"
          @click="performMutation"
        >
          {{ t("sftp.applyMutation") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      plugin-protected
      :model-value="permissionsTarget !== null"
      :title="t('sftp.permissions.title')"
      :description="permissionsTarget ? t('sftp.permissions.description', { name: permissionsTarget.displayName }) : ''"
      :close-label="t('sftp.permissions.close')"
      :dismissible="!permissionsPending"
      @update:model-value="(value) => { if (!value && !permissionsPending) permissionsTarget = null }"
    >
      <div class="sftp-view__permissions-grid">
        <div
          v-for="group in permissionGroups"
          :key="group.key"
          class="sftp-view__permissions-group"
        >
          <strong>{{ t(`sftp.permissions.${group.key}`) }}</strong>
          <NvxCheckbox
            v-for="(bit, index) in group.bits"
            :key="bit"
            class="sftp-view__permission-check"
            :model-value="Boolean(permissionMode & bit)"
            :disabled="permissionsPending"
            @update:model-value="setPermissionBit(bit, $event)"
          >
            {{ t(`sftp.permissions.${permissionActions[index]}`) }}
          </NvxCheckbox>
        </div>
      </div>
      <p class="sftp-view__permissions-mode">
        {{ t('sftp.permissions.mode', { mode: permissionModeLabel }) }}
      </p>
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="permissionsPending"
          @click="permissionsTarget = null"
        >
          {{ t("sftp.permissions.cancel") }}
        </NvxButton>
        <NvxButton
          variant="primary"
          :loading="permissionsPending"
          :disabled="permissionsPending || permissionMode === ((permissionsTarget?.expectedPermissionBits ?? 0) & 0o777)"
          @click="applyPermissions"
        >
          {{ t("sftp.permissions.apply") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      plugin-protected
      :model-value="fileUtilityDialog !== null"
      :title="fileUtilityDialog ? t(`sftp.fileUtilities.${fileUtilityDialog}.title`) : ''"
      :description="fileUtilityDialog ? t(`sftp.fileUtilities.${fileUtilityDialog}.description`) : ''"
      :close-label="t('sftp.closeMutation')"
      :dismissible="!fileUtilityPanePending"
      @update:model-value="(value) => { if (!value && !fileUtilityPanePending) { fileUtilityDialog = null; fileUtilityTargetPaneId = null } }"
    >
      <NvxInlineNotice
        tone="info"
        :title="t('sftp.fileUtilities.coreProcessingTitle')"
      >
        {{ t("sftp.fileUtilities.coreProcessingBody") }}
      </NvxInlineNotice>
      <NvxField
        v-if="fileUtilityDialog === 'downloadUrl'"
        for-id="sftp-file-utility-url"
        :label="t('sftp.sourceUrl')"
      >
        <NvxInput
          id="sftp-file-utility-url"
          v-model="fileUtilityUrl"
          inputmode="url"
          :invalid="fileUtilityUrl.length > 8 && !fileUtilityValid"
          @blur="inferNetworkFileName"
        />
      </NvxField>
      <NvxField
        for-id="sftp-file-utility-name"
        :label="fileUtilityDialog === 'extract' ? t('sftp.targetFolderName') : t('sftp.targetFileName')"
      >
        <NvxInput
          id="sftp-file-utility-name"
          v-model="fileUtilityName"
          :invalid="!fileUtilityNameValid && fileUtilityName.length > 0"
        />
      </NvxField>
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="fileUtilityPanePending"
          @click="fileUtilityDialog = null; fileUtilityTargetPaneId = null"
        >
          {{ t("sftp.cancelMutation") }}
        </NvxButton>
        <NvxButton
          :loading="fileUtilityPanePending"
          :disabled="fileUtilityPanePending || !fileUtilityValid"
          @click="performFileUtility"
        >
          {{ t("sftp.applyMutation") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      plugin-protected
      :model-value="cleanupRetainTarget !== null"
      :title="t('sftp.retainRemoteDialog.title')"
      :description="t('sftp.retainRemoteDialog.description')"
      :close-label="t('sftp.retainRemoteDialog.close')"
      :dismissible="cleanupPendingTransferId === null"
      @update:model-value="(value) => { if (!value) cleanupRetainTarget = null }"
    >
      <NvxInlineNotice
        tone="warning"
        :title="t('sftp.retainRemoteDialog.warning')"
      >
        {{ cleanupRetainTarget ? cleanupResidualDisplay(cleanupRetainTarget) : "" }}
      </NvxInlineNotice>
      <template #actions>
        <NvxButton
          variant="secondary"
          @click="cleanupRetainTarget = null"
        >
          {{ t("sftp.retainRemoteDialog.cancel") }}
        </NvxButton><NvxButton
          variant="danger"
          :loading="cleanupPendingTransferId !== null"
          @click="confirmRetainRemoteCleanup"
        >
          {{ t("sftp.retainRemoteDialog.confirm") }}
        </NvxButton>
      </template>
    </NvxDialog>
  </main>
</template>

<style scoped>
.sftp-view { display:flex; flex-direction:column; width:100%; height:100%; min-width:0; min-height:0; overflow:hidden; background:var(--nvx-color-bg-surface); }
.sftp-view__topbar,.sftp-view__title,.sftp-view__pane-header,.sftp-view__pane-header>div,.sftp-view__pane-controls,.sftp-view__commandbar,.sftp-view__command-path,.sftp-view__transfers>header,.sftp-view__transfers>header>div,.sftp-view__transfer-title,.sftp-view__transfer-actions { display:flex; gap:var(--nvx-space-2); align-items:center; }
.sftp-view__topbar { position:relative; z-index:var(--nvx-z-sticky); display:grid; flex:0 0 auto; grid-template-columns:max-content max-content; justify-content:space-between; gap:var(--nvx-space-3); min-height:56px; padding:var(--nvx-space-2) var(--nvx-space-4); border-bottom:var(--nvx-border-width) solid var(--nvx-color-border); background:var(--nvx-color-bg-surface); }
.sftp-view__title { min-width:0; }.sftp-view__title h1 { margin:0; font-size:var(--nvx-font-size-md); white-space:nowrap; }.sftp-view__host-control { flex:1 1 220px; width:min(300px,100%); min-width:150px; }.sftp-view__transfer-toggle { position:relative; overflow:hidden; white-space:nowrap; }.sftp-view__transfer-toggle-percent { color:var(--nvx-color-accent); font-variant-numeric:tabular-nums; }.sftp-view__transfer-toggle-track { position:absolute; right:var(--nvx-space-2); bottom:2px; left:var(--nvx-space-2); height:2px; overflow:hidden; border-radius:var(--nvx-radius-sm); background:var(--nvx-color-border-subtle); }.sftp-view__transfer-toggle-track>span { display:block; height:100%; background:var(--nvx-color-accent); }
.sftp-view__transfer-activity { position:relative; justify-self:end; }
.sftp-view__browser { display:block; flex:1 1 auto; min-width:0; min-height:0; overflow:hidden; background:var(--nvx-color-bg-surface); }
.sftp-view__workspace { width:100%; height:100%; min-width:0; min-height:0; --nvx-color-terminal-bg:var(--nvx-color-bg-surface); --nvx-color-terminal-pane-active-border:var(--nvx-color-border-strong); }
.sftp-view__pane { container-name:sftp-file-pane; container-type:inline-size; display:grid; grid-template-columns:minmax(0,1fr); grid-template-rows:auto auto auto minmax(0,1fr); width:100%; height:100%; min-width:0; min-height:0; overflow:hidden; background:var(--nvx-color-bg-surface); }.sftp-view__pane.is-drop-target { box-shadow:inset 0 0 0 2px var(--nvx-color-accent); background:var(--nvx-color-accent-soft); }
.sftp-view__pane-header { min-height:50px; padding:var(--nvx-space-2) var(--nvx-space-3); border-bottom:var(--nvx-border-width) solid var(--nvx-color-border); }.sftp-view__pane-identity { min-width:112px; }.sftp-view__pane-identity strong { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }.sftp-view__pane-endpoint { flex:1 1 auto; min-width:0; justify-content:flex-end; }.sftp-view__pane-controls { flex:0 0 auto; }.sftp-view__pane-actions-menu { display:inline-flex; }
.sftp-view__commandbar { position:relative; min-height:48px; padding:var(--nvx-space-1) var(--nvx-space-3); border-bottom:var(--nvx-border-width) solid var(--nvx-color-border-subtle); }
.sftp-view__command-separator { flex:0 0 1px; align-self:stretch; min-height:24px; margin-block:var(--nvx-space-1); background:var(--nvx-color-border-subtle); }
.sftp-view__command-path { position:relative; flex:1 1 auto; min-width:96px; height:var(--nvx-control-height-sm); padding-inline:var(--nvx-space-2); border:var(--nvx-border-width) solid var(--nvx-color-border); border-radius:var(--nvx-radius-sm); background:var(--nvx-color-bg-surface); }
.sftp-view__command-path>:deep(svg) { flex:0 0 auto; color:var(--nvx-color-text-secondary); }
.sftp-view__command-path>:deep(.nvx-input) { min-width:0; height:calc(var(--nvx-control-height-sm) - 2px); min-height:calc(var(--nvx-control-height-sm) - 2px); padding:0; border:0; border-radius:0; background:transparent; box-shadow:none; }
.sftp-view__command-path:focus-within { border-color:var(--nvx-color-accent); box-shadow:0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring); }
.sftp-view__path-suggestions { position:absolute; z-index:var(--nvx-z-popover); top:calc(100% + var(--nvx-space-2)); left:0; width:max(220px,100%); max-width:min(360px,calc(100cqw - 24px)); max-height:224px; margin:0; padding:var(--nvx-space-1); overflow:auto; border:var(--nvx-border-width) solid var(--nvx-color-border); border-radius:var(--nvx-radius-sm); background:var(--nvx-color-bg-surface); box-shadow:var(--nvx-shadow-overlay); list-style:none; }
.sftp-view__path-suggestions button { width:100%; min-height:32px; padding:0 var(--nvx-space-2); overflow:hidden; border:0; border-radius:var(--nvx-radius-xs); background:transparent; color:var(--nvx-color-text-primary); font:inherit; text-align:start; text-overflow:ellipsis; white-space:nowrap; }.sftp-view__path-suggestions button:hover,.sftp-view__path-suggestions button.is-active { background:var(--nvx-color-bg-hover); }.sftp-view__path-suggestions button:focus-visible { outline:var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring); }
.sftp-view__command-search { position:relative; flex:0 0 auto; }
.sftp-view__command-search .is-active { color:var(--nvx-color-accent); background:var(--nvx-color-accent-soft); }
.sftp-view__search-popover { position:absolute; z-index:var(--nvx-z-popover); top:calc(100% + var(--nvx-space-2)); right:0; width:min(240px,calc(100cqw - 24px)); box-shadow:var(--nvx-shadow-overlay); }
.sftp-view__command-sort { position:relative; display:inline-flex; flex:0 0 var(--nvx-control-height-sm); width:var(--nvx-control-height-sm); height:var(--nvx-control-height-sm); overflow:hidden; }
.sftp-view__command-sort :deep(.nvx-select),.sftp-view__command-sort :deep(.nvx-select__trigger) { width:var(--nvx-control-height-sm); height:var(--nvx-control-height-sm); min-height:var(--nvx-control-height-sm); }
.sftp-view__command-sort :deep(.nvx-select__trigger) { justify-content:center; padding:0; overflow:hidden; border-color:transparent; background:transparent; }
.sftp-view__command-sort :deep(.nvx-select__trigger:hover:not(:disabled)) { background:var(--nvx-color-bg-hover); }
.sftp-view__command-sort :deep(.nvx-select__value),.sftp-view__command-sort :deep(.nvx-select__chevron) { position:absolute; width:1px; height:1px; padding:0; overflow:hidden; clip:rect(0,0,0,0); white-space:nowrap; border:0; }
.sftp-view__command-sort>svg { position:absolute; inset:50% auto auto 50%; pointer-events:none; transform:translate(-50%,-50%); color:var(--nvx-color-text-secondary); }
.sftp-view__columns,.sftp-view__entries>button { display:grid; grid-template-columns:minmax(160px,1fr) minmax(84px,.28fr) minmax(112px,.36fr); gap:var(--nvx-space-3); align-items:center; }.sftp-view__columns { min-height:28px; padding:0 var(--nvx-space-3); border-bottom:var(--nvx-border-width) solid var(--nvx-color-border); color:var(--nvx-color-text-tertiary); font-size:var(--nvx-font-size-xs); }
.sftp-view__entries { min-height:0; overflow:auto; }.sftp-view__entries>button { width:100%; min-height:34px; padding:0 var(--nvx-space-3); border:0; border-bottom:var(--nvx-border-width) solid var(--nvx-color-border-subtle); background:transparent; color:var(--nvx-color-text-primary); text-align:start; }.sftp-view__entries>button:hover { background:var(--nvx-color-bg-hover); }.sftp-view__entries>button.is-selected { background:var(--nvx-color-accent-soft); }.sftp-view__entries>button:focus-visible { outline:var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring); outline-offset:-2px; }
.sftp-view__selection-count { font:inherit; color:var(--nvx-color-accent); }
.sftp-view__entry-name { display:flex; gap:var(--nvx-space-2); align-items:center; min-width:0; }.sftp-view__entry-name span,.sftp-view__entries small { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }.sftp-view__entries small { color:var(--nvx-color-text-secondary); font-variant-numeric:tabular-nums; }.sftp-view__entries>p { margin:0; padding:var(--nvx-space-4); color:var(--nvx-color-text-secondary); }
.sftp-view__pane-error { display:grid; justify-items:start; gap:var(--nvx-space-2); padding:var(--nvx-space-4); color:var(--nvx-color-text-secondary); }.sftp-view__pane-error p { margin:0; }
.sftp-view__context-menu { box-sizing:border-box; max-width:calc(100vw - 16px); max-height:calc(100dvh - 16px); overflow-y:auto; overscroll-behavior:contain; grid-auto-rows:max-content; position:fixed; z-index:var(--nvx-z-popover); display:grid; width:216px; padding:var(--nvx-space-1); border:var(--nvx-border-width) solid var(--nvx-color-border-strong); border-radius:var(--nvx-radius-md); background:var(--nvx-color-bg-surface); box-shadow:var(--nvx-shadow-overlay); }.sftp-view__context-menu button { display:flex; align-items:center; gap:var(--nvx-space-2); min-height:36px; padding:0 var(--nvx-space-3); border:0; border-radius:var(--nvx-radius-sm); background:transparent; color:var(--nvx-color-text-primary); font:inherit; text-align:start; }.sftp-view__context-menu button:hover,.sftp-view__context-menu button:focus-visible { background:var(--nvx-color-bg-hover); outline:none; }.sftp-view__context-menu span[role="separator"] { height:var(--nvx-border-width); margin:var(--nvx-space-1) var(--nvx-space-2); background:var(--nvx-color-border-subtle); }.sftp-view__context-menu button.sftp-view__context-menu-danger { color:var(--nvx-color-danger); }
.sftp-view__context-menu button:disabled { cursor:not-allowed; opacity:.48; }.sftp-view__context-menu button:disabled:hover { background:transparent; }
.sftp-view__permissions-grid { display:grid; grid-template-columns:repeat(3,minmax(0,1fr)); gap:var(--nvx-space-2); }
.sftp-view__permissions-group { display:grid; align-content:start; gap:var(--nvx-space-1); min-width:0; }
.sftp-view__permissions-group>strong { margin-bottom:var(--nvx-space-1); font-size:var(--nvx-font-size-sm); }
.sftp-view__permission-check { padding:var(--nvx-space-1); }
.sftp-view__permissions-mode { margin:var(--nvx-space-3) 0 0; color:var(--nvx-color-text-secondary); font-variant-numeric:tabular-nums; }
.sftp-view__preview { display:grid; place-items:center; min-height:280px; max-height:min(62vh,560px); overflow:auto; border:var(--nvx-border-width) solid var(--nvx-color-border); border-radius:var(--nvx-radius-sm); background:var(--nvx-color-bg-subtle); }.sftp-view__preview>p { margin:0; color:var(--nvx-color-text-secondary); }.sftp-view__preview img { display:block; max-width:100%; max-height:min(62vh,560px); object-fit:contain; }
.sftp-view__preview.is-text { display:flex; flex-direction:column; align-items:stretch; width:100%; height:min(62vh,500px); overflow:hidden; background:var(--nvx-color-bg-surface); }.sftp-view__preview-toolbar { display:flex; flex:0 0 auto; align-items:center; gap:var(--nvx-space-2); min-height:44px; padding:var(--nvx-space-2) var(--nvx-space-3); border-bottom:var(--nvx-border-width) solid var(--nvx-color-border); }.sftp-view__preview-toolbar>span:first-child { margin-inline-end:auto; color:var(--nvx-color-text-secondary); font-size:var(--nvx-font-size-sm); }.sftp-view__preview.is-text>:deep(.nvx-inline-notice) { flex:0 0 auto; margin:var(--nvx-space-2); }.sftp-view__code-editor { flex:1 1 auto; min-height:0; }
.sftp-view__transfers { position:absolute; inset-block-start:calc(100% + var(--nvx-space-2)); inset-inline-end:0; z-index:var(--nvx-z-popover); display:grid; gap:var(--nvx-space-2); width:min(500px,calc(100vw - 112px)); max-height:min(500px,calc(100vh - 132px)); padding:var(--nvx-space-3); overflow:auto; border:var(--nvx-border-width) solid var(--nvx-color-border); border-radius:var(--nvx-radius-md); background:var(--nvx-color-bg-surface); box-shadow:var(--nvx-shadow-overlay); }.sftp-view__transfers.is-empty { width:min(360px,calc(100vw - 112px)); }.sftp-view__transfers>header { justify-content:space-between; }.sftp-view__transfers>header>div { min-width:0; }.sftp-view__transfers h2 { margin:0; font-size:var(--nvx-font-size-md); }.sftp-view__transfers header span { color:var(--nvx-color-text-tertiary); font-size:var(--nvx-font-size-xs); white-space:nowrap; }.sftp-view__transfer-empty { margin:0; color:var(--nvx-color-text-secondary); }.sftp-view__transfer { display:grid; gap:var(--nvx-space-1); min-width:0; padding-top:var(--nvx-space-2); border-top:var(--nvx-border-width) solid var(--nvx-color-border-subtle); }.sftp-view__transfer-title { min-width:0; }.sftp-view__transfer-title strong { margin-inline-end:auto; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }.sftp-view__transfer-meta { display:flex; flex-wrap:wrap; gap:var(--nvx-space-2); color:var(--nvx-color-text-secondary); font-size:var(--nvx-font-size-xs); font-variant-numeric:tabular-nums; }.sftp-view__transfer-meta:empty { display:none; }.sftp-view__transfer-actions { flex-wrap:wrap; justify-content:flex-end; }
@container sftp-file-pane (max-width:620px) {
  .sftp-view__pane-header { display:grid; grid-template-columns:minmax(0,1fr) max-content; }
  .sftp-view__pane-identity { grid-column:1; }
  .sftp-view__pane-controls { grid-column:2; grid-row:1; }
  .sftp-view__pane-endpoint { grid-column:1/-1; grid-row:2; flex-wrap:wrap; justify-content:flex-start; }
  .sftp-view__host-control { flex:1 1 180px; width:auto; }
}
@container sftp-file-pane (max-width:480px) {
  .sftp-view__pane-header,.sftp-view__commandbar { padding-inline:var(--nvx-space-2); }
  .sftp-view__pane-controls { gap:var(--nvx-space-1); }
  .sftp-view__columns,.sftp-view__entries>button { grid-template-columns:minmax(0,1fr) minmax(72px,.32fr); gap:var(--nvx-space-2); padding-inline:var(--nvx-space-2); }
  .sftp-view__columns>:nth-child(3),.sftp-view__entries>button>:nth-child(3) { display:none; }
}
@container sftp-file-pane (max-width:340px) {
  .sftp-view__pane-endpoint { flex-wrap:wrap; }
  .sftp-view__pane-endpoint>:deep(.nvx-status-label) { order:-1; }
  .sftp-view__columns,.sftp-view__entries>button { grid-template-columns:minmax(0,1fr); }
  .sftp-view__columns>:nth-child(2),.sftp-view__entries>button>:nth-child(2) { display:none; }
}
@media (max-width:1100px) { .sftp-view__topbar { gap:var(--nvx-space-2); padding-inline:var(--nvx-space-3); } }
.sftp-view__transfer--focused { outline: 2px solid var(--nvx-color-border); outline-offset: 4px; }
</style>
