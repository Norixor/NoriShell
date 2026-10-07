<script setup lang="ts">
import { FilePlus2, FolderOpen, Monitor, Network, Plug, Plus, SquareTerminal, Terminal, X } from "lucide-vue-next";
import { computed, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import { useTipsStore } from "../../stores/tips";
import {
  activateSingleDocumentTab,
  activeSingleDocumentTabId,
  closeSingleDocumentTab,
  createSingleDocumentTab,
  useSingleDocumentTabs,
  type SingleDocumentTab,
} from "../../single-document-workspace";
import { NvxDialog, NvxIcon, NvxIconButton } from "../ui";

/**
 * Phone Tab switcher: a tall bottom sheet listing every Workspace Tab of this
 * document with its type, state and Pane count. Activation and close go through
 * the owning view's controller, so closing a Tab keeps the desktop confirmation
 * semantics (a last running attachment still asks before disconnecting).
 */
defineProps<{ modelValue: boolean }>();
const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();
const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const tips = useTipsStore();
const { store, tabs } = useSingleDocumentTabs();
const activeId = computed(() => activeSingleDocumentTabId(store, route.path));
const busy = computed(() => store.terminalBusy || store.desktopBusy);

function iconFor(tab: SingleDocumentTab): Component {
  if (tab.kind === "file") return FolderOpen;
  if (tab.kind === "desktop") return Monitor;
  if (tab.kind === "page") return tab.route.startsWith("/plugin/") ? Plug : FilePlus2;
  if (tab.terminalKind === "local") return Terminal;
  if (tab.terminalKind === "telnet") return Network;
  if (tab.terminalKind === "plugin") return Plug;
  return SquareTerminal;
}

function kindLabel(tab: SingleDocumentTab) {
  if (tab.kind === "file") return t("navigation.sftp");
  if (tab.kind === "desktop") return t("desktop.navigation");
  if (tab.kind === "page") return t("workspaceTabs.pageState");
  return t(`mobile.tabKinds.${tab.terminalKind ?? "ssh"}`);
}

function titleFor(tab: SingleDocumentTab) {
  return tab.labelKey ? t(tab.labelKey) : tab.label;
}

function stateFor(tab: SingleDocumentTab) {
  if (tab.kind === "file") return t("sshTerminal.paneCount", tab.paneCount);
  return tab.stateLabel || kindLabel(tab);
}

function report(scope: string, titleKey: string) {
  tips.show({ scope, tone: "error", title: t(titleKey) });
}

function dismiss() {
  emit("update:modelValue", false);
}

function activate(tab: SingleDocumentTab) {
  dismiss();
  void activateSingleDocumentTab(store, router, tab).catch(() => report("mobile-tab-open", "workspaceTabs.openFailed"));
}

function close(tab: SingleDocumentTab) {
  // The sheet stays open so the user can keep managing Tabs; a declined confirmation keeps the Tab.
  void closeSingleDocumentTab(store, router, tab).catch(() => report("mobile-tab-close", "workspaceTabs.closeFailed"));
}

function create() {
  dismiss();
  void createSingleDocumentTab(store, router).catch(() => report("mobile-tab-open", "workspaceTabs.openFailed"));
}
</script>

<template>
  <NvxDialog
    :model-value="modelValue"
    :title="t('mobile.switcherTitle')"
    :close-label="t('mobile.closeSheet')"
    sheet-size="tall"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <ul
      class="nvx-mobile-switcher"
      :aria-label="t('workspaceTabs.label')"
    >
      <li>
        <button
          class="nvx-mobile-switcher__row nvx-mobile-switcher__row--new"
          type="button"
          data-nvx-dialog-initial-focus
          :disabled="busy"
          @click="create"
        >
          <span class="nvx-mobile-switcher__icon nvx-mobile-switcher__icon--new">
            <NvxIcon
              :icon="Plus"
              :size="20"
            />
          </span>
          <span class="nvx-mobile-switcher__text">
            <strong>{{ t('mobile.newTab') }}</strong>
          </span>
        </button>
      </li>
      <li
        v-if="!tabs.length"
        class="nvx-mobile-switcher__empty"
      >
        {{ t('mobile.switcherEmpty') }}
      </li>
      <li
        v-for="tab in tabs"
        :key="tab.id"
        class="nvx-mobile-switcher__item"
        :class="{ 'nvx-mobile-switcher__item--active': tab.id === activeId }"
      >
        <button
          class="nvx-mobile-switcher__row"
          type="button"
          :aria-current="tab.id === activeId ? 'true' : undefined"
          :aria-label="`${titleFor(tab)}, ${stateFor(tab)}`"
          :disabled="busy"
          @click="activate(tab)"
        >
          <span class="nvx-mobile-switcher__icon">
            <NvxIcon
              :icon="iconFor(tab)"
              :size="20"
            />
          </span>
          <span class="nvx-mobile-switcher__text">
            <strong>{{ titleFor(tab) }}</strong>
            <span class="nvx-mobile-switcher__state">
              <span
                v-if="tab.kind !== 'page'"
                class="nvx-mobile-switcher__dot"
                :class="`nvx-mobile-switcher__dot--${tab.stateTone}`"
                aria-hidden="true"
              />
              {{ stateFor(tab) }}
              <span
                v-if="tab.bellAttention"
                class="nvx-mobile-switcher__bell"
              >· {{ t('terminalInteraction.bellAttention') }}</span>
            </span>
          </span>
        </button>
        <NvxIconButton
          class="nvx-mobile-switcher__close"
          :label="t('mobile.closeTab', { label: titleFor(tab) })"
          :disabled="busy"
          @click="close(tab)"
        >
          <NvxIcon
            :icon="X"
            :size="20"
          />
        </NvxIconButton>
      </li>
    </ul>
  </NvxDialog>
</template>

<style scoped>
.nvx-mobile-switcher {
  display: grid;
  align-content: start;
  gap: var(--nvx-space-1);
  margin: 0 calc(-1 * var(--nvx-space-2));
  padding: 0;
  list-style: none;
}

.nvx-mobile-switcher__item {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  border-radius: var(--nvx-radius-md);
}

.nvx-mobile-switcher__item--active {
  background: var(--nvx-color-accent-soft);
}

.nvx-mobile-switcher__row {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  width: 100%;
  min-height: 56px;
  padding: var(--nvx-space-2) var(--nvx-space-3);
  border: 0;
  border-radius: var(--nvx-radius-md);
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  text-align: left;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.nvx-mobile-switcher__row:disabled { cursor: default; opacity: 0.6; }
.nvx-mobile-switcher__row:active:not(:disabled) { background: var(--nvx-color-bg-hover); }
.nvx-mobile-switcher__row:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: -2px;
}

.nvx-mobile-switcher__row--new { color: var(--nvx-color-accent); }

.nvx-mobile-switcher__icon {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-secondary);
}

.nvx-mobile-switcher__icon--new {
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}

.nvx-mobile-switcher__item--active .nvx-mobile-switcher__icon {
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-accent);
}

.nvx-mobile-switcher__text {
  display: grid;
  gap: 2px;
  min-width: 0;
}

.nvx-mobile-switcher__text strong {
  overflow: hidden;
  font-weight: var(--nvx-font-weight-medium);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-mobile-switcher__state {
  display: flex;
  gap: var(--nvx-space-1);
  align-items: center;
  overflow: hidden;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-mobile-switcher__dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--nvx-color-text-tertiary);
}

.nvx-mobile-switcher__dot--info { background: var(--nvx-color-accent); }
.nvx-mobile-switcher__dot--success { background: var(--nvx-color-success); }
.nvx-mobile-switcher__dot--warning { background: var(--nvx-color-warning); }
.nvx-mobile-switcher__dot--danger { background: var(--nvx-color-danger); }

.nvx-mobile-switcher__bell { color: var(--nvx-color-warning); }

.nvx-mobile-switcher__close {
  margin-right: var(--nvx-space-1);
}

.nvx-mobile-switcher__empty {
  padding: var(--nvx-space-4) var(--nvx-space-3);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}
</style>
