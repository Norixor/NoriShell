<script setup lang="ts">
import {
  FilePlus2,
  FolderOpen,
  Monitor,
  PanelRightClose,
  PanelRightOpen,
  SquareTerminal,
} from "lucide-vue-next";
import { storeToRefs } from "pinia";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import NvxTerminalTabBar, {
  type TerminalTabItem,
} from "../terminal/NvxTerminalTabBar.vue";
import { NvxPluginExtensionTarget } from "../plugins";
import { NvxIcon, NvxIconButton } from "../ui";
import {
  isEditableShortcutTarget,
  isShortcutExecutionAllowed,
  matchShortcut,
  shouldConsumeShortcut,
  SHORTCUT_COMMANDS,
  type ShortcutCommand,
  type ShortcutPlatform,
} from "../../shortcuts";
import { detectDesktopPlatform, shortcutProfilePlatform } from "../../platform";
import { useHostMarkersStore } from "../../stores/hostMarkers";
import { useShortcutsStore } from "../../stores/shortcuts";
import { useWorkspaceTabsStore } from "../../stores/workspaceTabs";
import { useUiStore } from "../../stores/ui";
import { moveWorkspaceTab, moveWorkspaceTabToNewWindow } from "../../workspace-tab-transfer";
import { showWorkspaceTabFailure } from "../../workspace-tab-errors";
import {
  activateWorkspaceTabView,
  createManagedFileTab,
  createManagedPageForRoute,
  createManagedTerminalTab,
  requestCloseWorkspaceTabView,
} from "../../workspace-tab-view-shell";
import { activeWorkspaceTabViewId, pendingWorkspaceTabViewId, workspaceTabViewSummaries, workspaceTabViewSummary } from "../../workspace-tab-view-state";
import {
  beginWorkspaceTabDrag,
  cancelWorkspaceTabDrag,
  finishWorkspaceTabDrag,
  workspaceWindowLabel,
} from "../../workspace-tab-windows";
import { useTipsStore } from "../../stores/tips";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const workspaceTabs = useWorkspaceTabsStore();
const ui = useUiStore();
const tips = useTipsStore();
const shortcuts = useShortcutsStore();
const hostMarkers = useHostMarkersStore();
const { terminalActivationBlocked } = storeToRefs(workspaceTabs);
// Quick Commands belong to the active Terminal Tab WebView, never to the shell page it covers.
const activeTerminal = computed(() => {
  // A Tab still rendering has no writable terminal yet.
  if (activeWorkspaceTabViewId.value === pendingWorkspaceTabViewId.value) return null;
  const summary = activeWorkspaceTabViewId.value ? workspaceTabViewSummary(activeWorkspaceTabViewId.value) : null;
  return summary?.kind === "terminal" ? summary : null;
});
function toggleQuickCommands() {
  const summary = activeTerminal.value;
  if (!summary) return;
  void emitTo(summary.viewLabel, "workspace-tab-view-toggle-quick-commands", { id: summary.id })
    .catch((error: unknown) => openFailure(error));
}

interface PendingPointerDrag {
  id: string;
  pointerId: number;
  x: number;
  y: number;
}

interface ActivePointerDrag {
  id: string;
  nonce: string;
}

interface DragReleased {
  id: string;
  nonce: string;
  target: string | null;
  x: number;
  y: number;
}

let pendingDrag: PendingPointerDrag | null = null;
let activeDrag: ActivePointerDrag | null = null;
interface DragHover { id: string; nonce: string; active: boolean }
const incomingDrag = ref<DragHover | null>(null);
let stopDragHover: UnlistenFn | null = null;
let mounted = false;
let stopDragReleased: UnlistenFn | null = null;
let dragReleaseReady = false;
let stopChildShortcuts: UnlistenFn | null = null;
let batchClosePending = false;

function tabActionFailure(action: "move" | "close" | "open", error?: unknown) {
  if (action === "move" && tips.items.some((item) => item.scope === "workspace-tab-diagnostic")) return;
  showWorkspaceTabFailure(error, `workspace-tab-${action}`, `workspace_tab.${action}_failed`, `workspaceTabs.${action}Failed`);
}

const moveFailure = (error?: unknown) => tabActionFailure("move", error);
const closeFailure = (error?: unknown) => tabActionFailure("close", error);
const openFailure = (error?: unknown) => tabActionFailure("open", error);

function tabPointerDown(id: string, event: PointerEvent) {
  if (!dragReleaseReady || !workspaceTabViewSummary(id) || activeDrag || event.button !== 0) return;
  pendingDrag = { id, pointerId: event.pointerId, x: event.screenX, y: event.screenY };
}

function pointerMove(event: PointerEvent) {
  const pending = pendingDrag;
  if (!pending || pending.pointerId !== event.pointerId) return;
  if (!(event.buttons & 1)) { pendingDrag = null; return; }
  if (Math.hypot(event.screenX - pending.x, event.screenY - pending.y) < 6) return;
  pendingDrag = null;
  const current = { id: pending.id, nonce: crypto.randomUUID() };
  activeDrag = current;
  tips.dismissScope("workspace-tab-diagnostic");
  void beginWorkspaceTabDrag(current.id, current.nonce).catch((error: unknown) => {
    if (activeDrag !== current) return;
    activeDrag = null;
    void cancelWorkspaceTabDrag(current.nonce).catch(() => undefined);
    // Linux has no cross-window Tab drag; the Tab context menu moves Tabs between windows there.
    if (!["workspace_tab.drag_button_released", "workspace_tab.drag_unsupported"].includes(String(error))) moveFailure(error);
  });
}

function pointerUp(event: PointerEvent) {
  if (pendingDrag?.pointerId === event.pointerId) pendingDrag = null;
}

function cancelPointerDrag(event: KeyboardEvent) {
  if (event.key !== "Escape") return;
  pendingDrag = null;
  const current = activeDrag;
  if (!current) return;
  activeDrag = null;
  void cancelWorkspaceTabDrag(current.nonce).catch(() => undefined);
}

async function dragReleased(payload: DragReleased) {
  const current = activeDrag;
  if (!current || current.id !== payload.id || current.nonce !== payload.nonce) return;
  activeDrag = null;
  try {
    if (payload.target === workspaceWindowLabel()) return;
    if (payload.target) await moveWorkspaceTab(current.id, payload.target);
    else await moveWorkspaceTabToNewWindow(current.id, { x: payload.x, y: payload.y });
  } catch (error) {
    moveFailure(error);
  } finally {
    await finishWorkspaceTabDrag(current.nonce).catch(() => undefined);
  }
}

function moveToNewWindow(id: string) {
  if (!workspaceTabViewSummary(id)) return;
  tips.dismissScope("workspace-tab-diagnostic");
  void moveWorkspaceTabToNewWindow(id).catch(moveFailure);
}

function moveToMainWindow(id: string) {
  if (!workspaceTabViewSummary(id)) return;
  tips.dismissScope("workspace-tab-diagnostic");
  void moveWorkspaceTab(id, "main").catch(moveFailure);
}

// Every Header Tab is a native Tab WebView; shell pages are not Tabs.
const items = computed<TerminalTabItem[]>(() => workspaceTabViewSummaries.value.map((summary) => ({
  groupId: summary.id,
  label: summary.label,
  stateLabel: summary.stateLabel,
  icon: summary.kind === "terminal" ? SquareTerminal : summary.kind === "file" ? FolderOpen
    : summary.kind === "desktop" ? Monitor : FilePlus2,
  compact: summary.kind === "page",
  hostMarker: summary.hostId ? hostMarkers.visibleMarker(summary.hostId) : null,
  bellAttentionLabel: summary.bellAttention ? t("terminalInteraction.bellAttention") : undefined,
})));

const activeGroupId = computed(() => activeWorkspaceTabViewId.value ?? "");

function activateGroup(groupId: string) {
  void activateWorkspaceTabView(groupId).catch(openFailure);
}

function closeGroup(groupId: string) {
  void requestCloseWorkspaceTabView(groupId).catch(closeFailure);
}

/** One owner-window dialog covers every resource that may still be running when closure begins. */
async function confirmBatchClose(groupIds: string[]): Promise<boolean> {
  if (groupIds.every((id) => workspaceTabViewSummary(id)?.kind === "page")) return true;
  return ask(t("workspaceTabs.confirmBatchCloseBody", { tabs: groupIds.length }), {
    title: t("workspaceTabs.confirmBatchCloseTitle"), kind: "warning",
    okLabel: t("workspaceTabs.confirmBatchCloseAction"),
    cancelLabel: t("sshTerminal.cancel"),
  });
}

async function closeGroups(groupIds: string[]) {
  if (batchClosePending) return;
  if (!groupIds.length) return;
  batchClosePending = true;
  try {
    if (!await confirmBatchClose(groupIds)) return;
    // Resource Tabs close before Page Tabs; a failed close keeps the remaining Tabs.
    const ordered = [...groupIds.filter((id) => workspaceTabViewSummary(id)?.kind !== "page"),
      ...groupIds.filter((id) => workspaceTabViewSummary(id)?.kind === "page")];
    for (const id of ordered) {
      if (workspaceTabViewSummary(id) && !await requestCloseWorkspaceTabView(id, true)) return;
    }
  } catch (error) {
    closeFailure(error);
  } finally {
    batchClosePending = false;
  }
}

function createTerminal() {
  const pending = ui.newTerminalBehavior === "welcome"
    ? createManagedPageForRoute("/new")
    : ui.newTerminalBehavior === "sftpWelcome"
      ? createManagedFileTab("remote")
      : createManagedTerminalTab(ui.newTerminalBehavior === "localTerminal" ? "local" : "welcome");
  void pending.catch(openFailure);
}

function reserveWorkspaceShortcut(event: KeyboardEvent) {
  event.preventDefault();
  event.stopImmediatePropagation();
}

function shortcutPlatform(): ShortcutPlatform {
  if (navigator.platform.startsWith("Win")) return "windows";
  if (navigator.platform.startsWith("Mac")) return "macos";
  return shortcutProfilePlatform(detectDesktopPlatform());
}

function shortcutRecordingActive() {
  return Boolean(document.querySelector(".nvx-shortcut-settings__recorder"));
}

function dialogOpen() {
  return terminalActivationBlocked.value || Boolean(document.querySelector("[role='dialog']"));
}

function activateRelativeWorkspaceTab(offset: 1 | -1) {
  const available = items.value;
  if (!available.length) return;
  const activeIndex = available.findIndex((item) => item.groupId === activeGroupId.value);
  const nextIndex = activeIndex < 0
    ? 0
    : (activeIndex + offset + available.length) % available.length;
  const target = available[nextIndex];
  if (target) activateGroup(target.groupId);
}

function executeShortcut(command: ShortcutCommand) {
  switch (command.id) {
    case "navigation.overview": void router.push("/overview"); break;
    case "navigation.terminal": void router.push("/terminal"); break;
    case "navigation.hosts": void router.push("/hosts"); break;
    case "navigation.sftp": void router.push("/sftp"); break;
    case "navigation.tunnels": void router.push("/tunnels"); break;
    case "navigation.plugins": void router.push("/plugins"); break;
    case "navigation.settings": void router.push("/settings"); break;
    case "navigation.known-hosts": void router.push("/settings?section=knownHosts"); break;
    case "navigation.ssh-identities": void router.push("/settings?section=identities"); break;
    case "workspace.new": createTerminal(); break;
    case "workspace.close":
      if (activeGroupId.value) closeGroup(activeGroupId.value);
      break;
    case "workspace.next": activateRelativeWorkspaceTab(1); break;
    case "workspace.previous": activateRelativeWorkspaceTab(-1); break;
    case "workspace.new-local": void createManagedTerminalTab("local").catch(openFailure); break;
    default: {
      const tabMatch = command.id.match(/^workspace\.tab\.([1-9])$/);
      const target = tabMatch ? items.value[Number(tabMatch[1]) - 1] : undefined;
      if (target) activateGroup(target.groupId);
    }
  }
}

function handleWorkspaceShortcut(event: KeyboardEvent) {
  const recording = shortcutRecordingActive();
  if (recording) return;
  const modalOpen = dialogOpen();
  if (isEditableShortcutTarget(event.target) && !modalOpen) return;
  const platform = shortcutPlatform();
  const command = matchShortcut(event, platform, shortcuts.bindingsFor(platform));
  // Terminal and File Pane commands execute only inside the focused Tab WebView.
  const context = {
    modalOpen,
    shortcutRecording: recording,
  };
  if (!shouldConsumeShortcut(command, context)) return;
  reserveWorkspaceShortcut(event);
  if (command && isShortcutExecutionAllowed(command, event, context)) executeShortcut(command);
}

// `/new` is a Page Tab, never a shell page.
watch(
  () => route.path,
  (path) => {
    if (path !== "/new") return;
    void createManagedPageForRoute(path).then((created) => {
      if (created && route.path === path) {
        void router.replace(workspaceWindowLabel() === "main" ? "/terminal" : "/workspace-window");
      }
    }).catch(openFailure);
  },
  { immediate: true },
);

onMounted(() => {
  mounted = true;
  window.addEventListener("keydown", handleWorkspaceShortcut, { capture: true });
  window.addEventListener("keydown", cancelPointerDrag, { capture: true });
  window.addEventListener("pointermove", pointerMove, { capture: true });
  window.addEventListener("pointerup", pointerUp, { capture: true });
  window.addEventListener("pointercancel", pointerUp, { capture: true });
  if (!isTauri()) return;
  void listen<DragHover>("workspace-tab-drag-hover", ({ payload }) => {
    if (payload.active) incomingDrag.value = payload;
    else if (incomingDrag.value?.nonce === payload.nonce) incomingDrag.value = null;
  }).then((unlisten) => {
    if (mounted) stopDragHover = unlisten;
    else unlisten();
  }).catch(() => undefined);
  void listen<DragReleased>("workspace-tab-drag-released", ({ payload }) => {
    void dragReleased(payload);
  }).then((unlisten) => {
    if (mounted) {
      stopDragReleased = unlisten;
      dragReleaseReady = true;
    } else unlisten();
  }).catch(() => { if (mounted) moveFailure("workspace_tab.drag_listener_unavailable"); });
  void listen<{ id: string; viewLabel: string; commandId: string }>("workspace-tab-shortcut", ({ payload }) => {
    if (payload.id !== activeWorkspaceTabViewId.value
      || workspaceTabViewSummary(payload.id)?.viewLabel !== payload.viewLabel) return;
    const command = SHORTCUT_COMMANDS.find((item) => item.id === payload.commandId);
    if (command?.scope === "app" && !dialogOpen()) executeShortcut(command);
  }).then((unlisten) => { stopChildShortcuts = unlisten; }).catch(() => undefined);
});
onBeforeUnmount(() => {
  mounted = false;
  stopDragHover?.();
  incomingDrag.value = null;
  window.removeEventListener("keydown", handleWorkspaceShortcut, { capture: true });
  window.removeEventListener("keydown", cancelPointerDrag, { capture: true });
  window.removeEventListener("pointermove", pointerMove, { capture: true });
  window.removeEventListener("pointerup", pointerUp, { capture: true });
  window.removeEventListener("pointercancel", pointerUp, { capture: true });
  stopDragReleased?.();
  stopDragReleased = null;
  dragReleaseReady = false;
  stopChildShortcuts?.();
  pendingDrag = null;
  if (activeDrag) void cancelWorkspaceTabDrag(activeDrag.nonce).catch(() => undefined);
  activeDrag = null;
});
</script>

<template>
  <NvxTerminalTabBar
    id="nvx-workspace-tab-bar"
    :items="items"
    :model-value="activeGroupId"
    :label="t('workspaceTabs.label')"
    :new-label="t('newWorkspace.title')"
    :close-label="t('workspaceTabs.close')"
    :close-all-label="t('workspaceTabs.closeAll')"
    :close-left-label="t('workspaceTabs.closeLeft')"
    :close-right-label="t('workspaceTabs.closeRight')"
    :move-to-new-window-label="t('workspaceTabs.moveToNewWindow')"
    :move-to-main-window-label="isTauri() && workspaceWindowLabel() !== 'main' ? t('workspaceTabs.moveToMainWindow') : ''"
    :scroll-backward-label="t('sshTerminal.scrollTabsBackward')"
    :scroll-forward-label="t('sshTerminal.scrollTabsForward')"
    :drag-enabled="isTauri()"
    :incoming-drag="Boolean(incomingDrag && !items.some((item) => item.groupId === incomingDrag?.id))"
    @update:model-value="activateGroup"
    @create="createTerminal"
    @close="closeGroup"
    @close-many="closeGroups"
    @tab-pointer-down="tabPointerDown"
    @move-to-new-window="moveToNewWindow"
    @move-to-main-window="moveToMainWindow"
  >
    <template
      #trailing-actions
    >
      <NvxPluginExtensionTarget
        target-id="app.header.actions"
        :show-identity="false"
      />
      <NvxIconButton
        v-if="activeTerminal"
        :label="activeTerminal.quickCommandsOpen ? t('quickCommands.collapse') : t('quickCommands.show')"
        @click="toggleQuickCommands"
      >
        <NvxIcon
          :icon="activeTerminal.quickCommandsOpen ? PanelRightClose : PanelRightOpen"
          :size="20"
        />
      </NvxIconButton>
    </template>
  </NvxTerminalTabBar>
</template>
