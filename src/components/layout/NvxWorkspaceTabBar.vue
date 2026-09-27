<script setup lang="ts">
import {
  Fingerprint,
  FilePlus2,
  FolderOpen,
  Monitor,
  KeyRound,
  Plug,
  PanelRightClose,
  PanelRightOpen,
  SquareTerminal,
} from "lucide-vue-next";
import { storeToRefs } from "pinia";
import { computed, onBeforeUnmount, onMounted, watch } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
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
  type ShortcutCommand,
  type ShortcutPlatform,
} from "../../shortcuts";
import { detectDesktopPlatform } from "../../platform";
import { useHostMarkersStore } from "../../stores/hostMarkers";
import { useShortcutsStore } from "../../stores/shortcuts";
import { useWorkspaceTabsStore, type WorkspacePageType } from "../../stores/workspaceTabs";
import { useUiStore } from "../../stores/ui";
import { hiddenOutgoingWorkspaceTabs, moveWorkspaceTab, moveWorkspaceTabToNewWindow } from "../../workspace-tab-transfer";
import {
  beginWorkspaceTabDrag,
  cancelWorkspaceTabDrag,
  finishWorkspaceTabDrag,
  workspaceWindowLabel,
  type WorkspaceTabKind,
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
const {
  terminalTabs,
  activeTerminalTabId,
  terminalBusy,
  terminalActivationBlocked,
  quickCommandsOpen,
  pageTabs,
  terminalController,
  desktopTabs,
  activeDesktopTabId,
  desktopBusy,
  desktopController,
  fileTabs,
  activeFileTabId,
} = storeToRefs(workspaceTabs);

const filePaneShortcutIds = new Set([
  "terminal.split-right", "terminal.split-down", "terminal.focus-next-pane",
  "terminal.focus-previous-pane", "terminal.close-pane",
]);
const createDisabled = computed(() => terminalBusy.value
  && (ui.newTerminalBehavior === "terminalWelcome" || ui.newTerminalBehavior === "localTerminal"));

const pageIcons = {
  newPage: FilePlus2,
  knownHosts: Fingerprint,
  sshIdentities: KeyRound,
  plugin: Plug,
} satisfies Record<WorkspacePageType, typeof Fingerprint>;

interface PendingPointerDrag {
  id: string;
  kind: WorkspaceTabKind;
  pointerId: number;
  x: number;
  y: number;
}

interface ActivePointerDrag {
  id: string;
  kind: WorkspaceTabKind;
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
let stopDragReleased: UnlistenFn | null = null;

function tabKind(id: string): WorkspaceTabKind | null {
  if (terminalTabs.value.some((tab) => tab.groupId === id)) return "terminal";
  if (pageTabs.value.some((tab) => tab.groupId === id)) return "page";
  if (desktopTabs.value.some((tab) => tab.groupId === id)) return "desktop";
  if (fileTabs.value.some((tab) => tab.groupId === id)) return "file";
  return null;
}

function moveFailure() {
  tips.show({ scope: "workspace-tab-move", tone: "error", title: t("workspaceTabs.moveFailed") });
}

function tabPointerDown(id: string, event: PointerEvent) {
  const kind = tabKind(id);
  if (!isTauri() || !kind || activeDrag || event.button !== 0) return;
  pendingDrag = { id, kind, pointerId: event.pointerId, x: event.screenX, y: event.screenY };
}

function pointerMove(event: PointerEvent) {
  const pending = pendingDrag;
  if (!pending || pending.pointerId !== event.pointerId) return;
  if (!(event.buttons & 1)) { pendingDrag = null; return; }
  if (Math.hypot(event.screenX - pending.x, event.screenY - pending.y) < 6) return;
  pendingDrag = null;
  const current = { id: pending.id, kind: pending.kind, nonce: crypto.randomUUID() };
  activeDrag = current;
  void beginWorkspaceTabDrag(current.id, current.nonce).catch((error: unknown) => {
    if (activeDrag !== current) return;
    activeDrag = null;
    void cancelWorkspaceTabDrag(current.nonce).catch(() => undefined);
    if (String(error) !== "workspace_tab.drag_button_released") moveFailure();
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
    if (payload.target) await moveWorkspaceTab(current.id, current.kind, payload.target);
    else await moveWorkspaceTabToNewWindow(current.id, current.kind, { x: payload.x, y: payload.y });
  } catch {
    moveFailure();
  } finally {
    await finishWorkspaceTabDrag(current.nonce).catch(() => undefined);
  }
}

function moveToNewWindow(id: string) {
  const kind = tabKind(id);
  if (kind) void moveWorkspaceTabToNewWindow(id, kind).catch(moveFailure);
}

function moveToMainWindow(id: string) {
  const kind = tabKind(id);
  if (kind) void moveWorkspaceTab(id, kind, "main").catch(moveFailure);
}

const items = computed<TerminalTabItem[]>(() => [
  ...terminalTabs.value.map((tab) => ({
    ...tab,
    icon: SquareTerminal,
    hostMarker: tab.hostId ? hostMarkers.visibleMarker(tab.hostId) : null,
    bellAttentionLabel: tab.bellAttention ? t("terminalInteraction.bellAttention") : undefined,
    disabled: terminalBusy.value,
  })),
  ...fileTabs.value.map((tab) => ({
    groupId: tab.groupId,
    label: tab.label || t(tab.kind === "remote" ? "fileWorkspace.remoteTab" : "fileWorkspace.localTab"),
    stateLabel: `${t("fileWorkspace.tabState")} · ${t("sshTerminal.paneCount", { count: tab.paneCount })}`,
    icon: FolderOpen,
  })),
  ...pageTabs.value.map((tab) => ({
    groupId: tab.groupId,
    label: tab.labelKey ? t(tab.labelKey) : tab.label,
    stateLabel: t("workspaceTabs.pageState"),
    icon: pageIcons[tab.pageType],
  })),
  ...desktopTabs.value.map((tab) => ({ ...tab, icon: Monitor, disabled: desktopBusy.value })),
].filter((tab) => !hiddenOutgoingWorkspaceTabs.value.has(tab.groupId)));

function pageIsActive(page: { pageType: WorkspacePageType; route: string }): boolean {
  if (page.route === route.path) return true;
  return route.path === "/settings" && (
    (page.pageType === "knownHosts" && route.query.section === "knownHosts")
    || (page.pageType === "sshIdentities" && route.query.section === "identities")
  );
}

const activeGroupId = computed(() => {
  if (route.path === "/desktop") return activeDesktopTabId.value;
  if (route.path === "/sftp") return activeFileTabId.value;
  const page = pageTabs.value.find(pageIsActive);
  if (page) return page.groupId;
  return route.path === "/terminal" ? activeTerminalTabId.value : "";
});

function deactivateTerminalWorkspace() {
  terminalController.value?.deactivate();
}

function activateGroup(groupId: string) {
  if (fileTabs.value.some((tab) => tab.groupId === groupId)) {
    deactivateTerminalWorkspace();
    desktopController.value?.deactivate();
    workspaceTabs.activateFileTab(groupId);
    void router.push("/sftp");
    return;
  }
  if (desktopTabs.value.some((tab) => tab.groupId === groupId)) {
    deactivateTerminalWorkspace();
    desktopController.value?.activate(groupId);
    return;
  }
  desktopController.value?.deactivate();
  const page = pageTabs.value.find((tab) => tab.groupId === groupId);
  if (page) {
    deactivateTerminalWorkspace();
    void router.push(page.route);
    return;
  }
  terminalController.value?.activate(groupId);
}

function closeGroup(groupId: string) {
  if (fileTabs.value.some((tab) => tab.groupId === groupId)) {
    activateGroup(groupId);
    void workspaceTabs.requestCloseFileTab(groupId);
    return;
  }
  if (desktopTabs.value.some((tab) => tab.groupId === groupId)) {
    desktopController.value?.close(groupId);
    return;
  }
  const page = pageTabs.value.find((tab) => tab.groupId === groupId);
  if (!page) {
    terminalController.value?.close(groupId);
    return;
  }
  const closingActivePage = pageIsActive(page);
  workspaceTabs.closePageTab(groupId);
  if (!closingActivePage) return;
  if (activeTerminalTabId.value && terminalController.value) {
    terminalController.value.activate(activeTerminalTabId.value);
    return;
  }
  const fallbackPage = pageTabs.value.at(-1);
  void router.push(fallbackPage?.route ?? "/settings");
}

async function closeGroups(groupIds: string[]) {
  const targetIds = new Set(groupIds);
  if (!targetIds.size) return;
  const fileIds = fileTabs.value.filter((tab) => targetIds.has(tab.groupId)).map((tab) => tab.groupId);
  for (const id of fileIds) {
    activateGroup(id);
    if (!await workspaceTabs.requestCloseFileTab(id)) return;
  }
  const desktopIds = desktopTabs.value.filter((tab) => targetIds.has(tab.groupId)).map((tab) => tab.groupId);
  if (desktopIds.length) desktopController.value?.closeMany(desktopIds);
  const terminalIds = terminalTabs.value
    .filter((tab) => targetIds.has(tab.groupId))
    .map((tab) => tab.groupId);
  const closingActivePage = pageTabs.value.some((tab) => (
    targetIds.has(tab.groupId) && pageIsActive(tab)
  ));
  for (const page of pageTabs.value.filter((tab) => targetIds.has(tab.groupId))) {
    workspaceTabs.closePageTab(page.groupId);
  }
  if (terminalIds.length && terminalController.value) {
    terminalController.value.closeMany(terminalIds);
    return;
  }
  if (!closingActivePage) return;
  const fallbackTerminal = terminalTabs.value.find((tab) => !targetIds.has(tab.groupId));
  if (fallbackTerminal && terminalController.value) {
    terminalController.value.activate(fallbackTerminal.groupId);
    return;
  }
  void router.push(pageTabs.value.at(-1)?.route ?? "/settings");
}

function createTerminal() {
  desktopController.value?.deactivate();
  if (ui.newTerminalBehavior === "welcome") {
    deactivateTerminalWorkspace();
    workspaceTabs.ensurePageTabForRoute("/new");
    void router.push("/new");
    return;
  }
  if (ui.newTerminalBehavior === "sftpWelcome") {
    deactivateTerminalWorkspace();
    workspaceTabs.showFileWelcome();
    void router.push("/sftp");
    return;
  }
  if (terminalController.value) {
    terminalController.value.create();
    return;
  }
  workspaceTabs.queueTerminalCreation();
  void router.push("/terminal");
}

function reserveWorkspaceShortcut(event: KeyboardEvent) {
  event.preventDefault();
  event.stopImmediatePropagation();
}

function shortcutPlatform(): ShortcutPlatform {
  if (navigator.platform.startsWith("Win")) return "windows";
  return detectDesktopPlatform() === "windows" ? "windows" : "macos";
}

function shortcutRecordingActive() {
  return Boolean(document.querySelector(".nvx-shortcut-settings__recorder"));
}

function dialogOpen() {
  return terminalActivationBlocked.value || Boolean(document.querySelector("[role='dialog']"));
}

function activateRelativeWorkspaceTab(offset: 1 | -1) {
  const available = items.value.filter((item) => !item.disabled);
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
    case "workspace.new":
      if (!createDisabled.value) createTerminal();
      break;
    case "workspace.close": {
      const active = items.value.find((item) => item.groupId === activeGroupId.value);
      if (active && !active.disabled) closeGroup(active.groupId);
      break;
    }
    case "workspace.next": activateRelativeWorkspaceTab(1); break;
    case "workspace.previous": activateRelativeWorkspaceTab(-1); break;
    case "workspace.new-local": terminalController.value?.runShortcut?.(command.id); break;
    case "terminal.split-right":
    case "terminal.split-down":
    case "terminal.focus-next-pane":
    case "terminal.focus-previous-pane":
    case "terminal.close-pane":
      if (route.path === "/sftp") workspaceTabs.runFileShortcut(command.id);
      else terminalController.value?.runShortcut?.(command.id);
      break;
    case "terminal.toggle-quick-commands": terminalController.value?.toggleQuickCommands(); break;
    default: {
      const tabMatch = command.id.match(/^workspace\.tab\.([1-9])$/);
      if (tabMatch) {
        const index = Number(tabMatch[1]) - 1;
        const target = items.value[index];
        if (target && !target.disabled) activateGroup(target.groupId);
        break;
      }
      terminalController.value?.runShortcut?.(command.id);
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
  const context = {
    terminalActive: (route.path === "/terminal" && Boolean(terminalController.value))
      || (route.path === "/sftp" && Boolean(activeFileTabId.value) && Boolean(command && filePaneShortcutIds.has(command.id))),
    modalOpen,
    shortcutRecording: recording,
  };
  if (!shouldConsumeShortcut(command, context)) return;
  reserveWorkspaceShortcut(event);
  if (command && isShortcutExecutionAllowed(command, event, context)) executeShortcut(command);
}

watch(
  () => route.path,
  (path) => workspaceTabs.ensurePageTabForRoute(path),
  { immediate: true },
);

onMounted(() => {
  window.addEventListener("keydown", handleWorkspaceShortcut, { capture: true });
  window.addEventListener("keydown", cancelPointerDrag, { capture: true });
  window.addEventListener("pointermove", pointerMove, { capture: true });
  window.addEventListener("pointerup", pointerUp, { capture: true });
  window.addEventListener("pointercancel", pointerUp, { capture: true });
  if (!isTauri()) return;
  void listen<DragReleased>("workspace-tab-drag-released", ({ payload }) => {
    void dragReleased(payload);
  }).then((unlisten) => { stopDragReleased = unlisten; }).catch(() => undefined);
});
onBeforeUnmount(() => {
  window.removeEventListener("keydown", handleWorkspaceShortcut, { capture: true });
  window.removeEventListener("keydown", cancelPointerDrag, { capture: true });
  window.removeEventListener("pointermove", pointerMove, { capture: true });
  window.removeEventListener("pointerup", pointerUp, { capture: true });
  window.removeEventListener("pointercancel", pointerUp, { capture: true });
  stopDragReleased?.();
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
    :context-menu-label="t('workspaceTabs.actions')"
    :move-to-new-window-label="t('workspaceTabs.moveToNewWindow')"
    :move-to-main-window-label="isTauri() && workspaceWindowLabel() !== 'main' ? t('workspaceTabs.moveToMainWindow') : ''"
    :scroll-backward-label="t('sshTerminal.scrollTabsBackward')"
    :scroll-forward-label="t('sshTerminal.scrollTabsForward')"
    :create-disabled="createDisabled"
    :drag-enabled="isTauri()"
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
        v-if="route.path === '/terminal'"
        :label="quickCommandsOpen ? t('quickCommands.collapse') : t('quickCommands.show')"
        @click="terminalController?.toggleQuickCommands()"
      >
        <NvxIcon
          :icon="quickCommandsOpen ? PanelRightClose : PanelRightOpen"
          :size="20"
        />
      </NvxIconButton>
    </template>
  </NvxTerminalTabBar>
</template>
