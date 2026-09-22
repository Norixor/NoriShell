<script setup lang="ts">
import {
  Fingerprint,
  Monitor,
  KeyRound,
  Plug,
  PanelRightClose,
  PanelRightOpen,
  SquareTerminal,
} from "lucide-vue-next";
import { storeToRefs } from "pinia";
import { computed, onBeforeUnmount, onMounted, watch } from "vue";
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

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const workspaceTabs = useWorkspaceTabsStore();
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
} = storeToRefs(workspaceTabs);

const pageIcons = {
  knownHosts: Fingerprint,
  sshIdentities: KeyRound,
  plugin: Plug,
} satisfies Record<WorkspacePageType, typeof Fingerprint>;

const items = computed<TerminalTabItem[]>(() => [
  ...terminalTabs.value.map((tab) => ({
    ...tab,
    icon: SquareTerminal,
    hostMarker: tab.hostId ? hostMarkers.visibleMarker(tab.hostId) : null,
    bellAttentionLabel: tab.bellAttention ? t("terminalInteraction.bellAttention") : undefined,
    disabled: terminalBusy.value,
  })),
  ...pageTabs.value.map((tab) => ({
    groupId: tab.groupId,
    label: tab.labelKey ? t(tab.labelKey) : tab.label,
    stateLabel: t("workspaceTabs.pageState"),
    icon: pageIcons[tab.pageType],
  })),
  ...desktopTabs.value.map((tab) => ({ ...tab, icon: Monitor, disabled: desktopBusy.value })),
]);

const activeGroupId = computed(() => {
  if (route.path === "/desktop") return activeDesktopTabId.value;
  const page = pageTabs.value.find((tab) => tab.route === route.path);
  if (page) return page.groupId;
  return route.path === "/terminal" ? activeTerminalTabId.value : "";
});

function deactivateTerminalWorkspace() {
  terminalController.value?.deactivate();
}

function activateGroup(groupId: string) {
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
  if (desktopTabs.value.some((tab) => tab.groupId === groupId)) {
    desktopController.value?.close(groupId);
    return;
  }
  const page = pageTabs.value.find((tab) => tab.groupId === groupId);
  if (!page) {
    terminalController.value?.close(groupId);
    return;
  }
  const closingActivePage = route.path === page.route;
  workspaceTabs.closePageTab(groupId);
  if (!closingActivePage) return;
  if (activeTerminalTabId.value && terminalController.value) {
    terminalController.value.activate(activeTerminalTabId.value);
    return;
  }
  const fallbackPage = pageTabs.value.at(-1);
  void router.push(fallbackPage?.route ?? "/settings");
}

function closeGroups(groupIds: string[]) {
  const targetIds = new Set(groupIds);
  if (!targetIds.size) return;
  const desktopIds = desktopTabs.value.filter((tab) => targetIds.has(tab.groupId)).map((tab) => tab.groupId);
  if (desktopIds.length) desktopController.value?.closeMany(desktopIds);
  const terminalIds = terminalTabs.value
    .filter((tab) => targetIds.has(tab.groupId))
    .map((tab) => tab.groupId);
  const closingActivePage = pageTabs.value.some((tab) => (
    targetIds.has(tab.groupId) && tab.route === route.path
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
      if (!terminalBusy.value) createTerminal();
      break;
    case "workspace.close": {
      const active = items.value.find((item) => item.groupId === activeGroupId.value);
      if (active && !active.disabled) closeGroup(active.groupId);
      break;
    }
    case "workspace.next": activateRelativeWorkspaceTab(1); break;
    case "workspace.previous": activateRelativeWorkspaceTab(-1); break;
    case "workspace.new-local": terminalController.value?.runShortcut?.(command.id); break;
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
    terminalActive: route.path === "/terminal" && Boolean(terminalController.value),
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

onMounted(() => window.addEventListener("keydown", handleWorkspaceShortcut, { capture: true }));
onBeforeUnmount(() => window.removeEventListener("keydown", handleWorkspaceShortcut, { capture: true }));
</script>

<template>
  <NvxTerminalTabBar
    id="nvx-workspace-tab-bar"
    :items="items"
    :model-value="activeGroupId"
    :label="t('workspaceTabs.label')"
    :new-label="t('sshTerminal.newConnection')"
    :close-label="t('workspaceTabs.close')"
    :close-all-label="t('workspaceTabs.closeAll')"
    :close-left-label="t('workspaceTabs.closeLeft')"
    :close-right-label="t('workspaceTabs.closeRight')"
    :context-menu-label="t('workspaceTabs.actions')"
    :scroll-backward-label="t('sshTerminal.scrollTabsBackward')"
    :scroll-forward-label="t('sshTerminal.scrollTabsForward')"
    :create-disabled="terminalBusy"
    @update:model-value="activateGroup"
    @create="createTerminal"
    @close="closeGroup"
    @close-many="closeGroups"
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
