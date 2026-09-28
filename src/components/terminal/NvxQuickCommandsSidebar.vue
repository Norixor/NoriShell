<script setup lang="ts">
import {
  Check,
  Command,
  Copy,
  Ellipsis,
  Pencil,
  Play,
  Plus,
  Trash2,
} from "lucide-vue-next";
import { computed, onBeforeUnmount, ref } from "vue";
import { useI18n } from "vue-i18n";

import {
  MAX_QUICK_COMMAND_LABEL_LENGTH,
  MAX_QUICK_COMMAND_LENGTH,
  useQuickCommandsStore,
  type QuickCommand,
} from "../../stores/quickCommands";
import {
  canRunInFocusedTerminal,
  focusedTerminalLabel,
  runInFocusedTerminal,
} from "../../terminal-input-target";
import {
  NvxButton,
  NvxDialog,
  NvxField,
  NvxIcon,
  NvxIconButton,
  NvxInlineNotice,
  NvxInput,
  NvxTextarea,
} from "../ui";
import { usePopoverMenu } from "../ui/usePopoverMenu";

const { t } = useI18n();
const quickCommands = useQuickCommandsStore();
const editorOpen = ref(false);
const deleteOpen = ref(false);
const editingId = ref<string | null>(null);
const pendingDelete = ref<QuickCommand | null>(null);
const label = ref("");
const command = ref("");
const saveErrorKey = ref<string | null>(null);
const deleteErrorVisible = ref(false);
const runningId = ref<string | null>(null);
const copiedId = ref<string | null>(null);
const actionMessage = ref("");
const menuItem = ref<QuickCommand | null>(null);
const {
  rootRef,
  triggerRef,
  viewportPanelRef,
  viewportPanelStyle,
  open: menuOpen,
  closeMenu,
  toggleMenu,
  handleMenuKeyDown,
} = usePopoverMenu();
let actionMessageTimer: number | null = null;
let menuTrigger: HTMLElement | null = null;

const editorTitle = computed(() =>
  editingId.value
    ? t("quickCommands.editTitle")
    : t("quickCommands.addTitle"),
);

function openCreate() {
  closeMenu();
  editingId.value = null;
  label.value = "";
  command.value = "";
  saveErrorKey.value = null;
  editorOpen.value = true;
}

function openEdit(item: QuickCommand) {
  closeMenu();
  menuTrigger?.focus();
  editingId.value = item.id;
  label.value = item.label;
  command.value = item.command;
  saveErrorKey.value = null;
  editorOpen.value = true;
}

function save() {
  const result = quickCommands.save({
    id: editingId.value ?? undefined,
    label: label.value,
    command: command.value,
  });
  saveErrorKey.value = result === "saved" ? null : `quickCommands.errors.${result}`;
  if (result === "saved") editorOpen.value = false;
}

function requestDelete(item: QuickCommand) {
  closeMenu();
  menuTrigger?.focus();
  pendingDelete.value = item;
  deleteErrorVisible.value = false;
  deleteOpen.value = true;
}

function toggleActions(item: QuickCommand, event: MouseEvent) {
  const wasOpen = menuOpen.value && menuItem.value?.id === item.id;
  closeMenu();
  menuItem.value = item;
  menuTrigger = event.currentTarget as HTMLElement;
  triggerRef(menuTrigger);
  if (!wasOpen) toggleMenu();
}

function confirmDelete() {
  if (!pendingDelete.value) return;
  const result = quickCommands.remove(pendingDelete.value.id);
  deleteErrorVisible.value = result === "storage-error";
  if (result !== "removed") return;
  deleteOpen.value = false;
  pendingDelete.value = null;
}

function showActionMessage(message: string) {
  if (actionMessageTimer !== null) window.clearTimeout(actionMessageTimer);
  actionMessage.value = message;
  actionMessageTimer = window.setTimeout(() => {
    actionMessage.value = "";
    copiedId.value = null;
    actionMessageTimer = null;
  }, 2_000);
}

async function run(item: QuickCommand) {
  if (runningId.value || !canRunInFocusedTerminal.value) return;
  const targetLabel = focusedTerminalLabel.value ?? t("quickCommands.focusedTerminal");
  runningId.value = item.id;
  const result = await runInFocusedTerminal(item.command);
  runningId.value = null;
  if (result === "sent") {
    showActionMessage(t("quickCommands.runSent", {
      label: item.label,
      terminal: targetLabel,
    }));
    return;
  }
  showActionMessage(t(`quickCommands.${result === "unavailable" ? "runUnavailable" : "runFailed"}`));
}

async function copy(item: QuickCommand) {
  try {
    await navigator.clipboard.writeText(item.command);
    copiedId.value = item.id;
    showActionMessage(t("quickCommands.copied", { label: item.label }));
  } catch {
    showActionMessage(t("quickCommands.copyFailed"));
  }
}

onBeforeUnmount(() => {
  if (actionMessageTimer !== null) window.clearTimeout(actionMessageTimer);
});
</script>

<template>
  <aside
    :ref="rootRef"
    class="quick-commands"
    :aria-label="t('quickCommands.title')"
  >
    <header class="quick-commands__header">
      <h2>{{ t("quickCommands.title") }}</h2>
      <NvxIconButton
        :label="t('quickCommands.add')"
        size="sm"
        @click="openCreate"
      >
        <NvxIcon
          :icon="Plus"
          :size="16"
        />
      </NvxIconButton>
    </header>
    <p
      v-if="actionMessage"
      class="quick-commands__feedback"
      role="status"
    >
      {{ actionMessage }}
    </p>
    <div
      v-if="quickCommands.commands.length"
      class="quick-commands__list"
    >
      <article
        v-for="item in quickCommands.commands"
        :key="item.id"
        class="quick-command"
      >
        <div class="quick-command__content">
          <strong :title="item.label">{{ item.label }}</strong>
          <code :title="item.command">{{ item.command }}</code>
        </div>
        <div class="quick-command__actions">
          <NvxButton
            class="quick-command__run"
            variant="ghost"
            size="sm"
            :aria-label="t('quickCommands.run', { label: item.label })"
            :title="canRunInFocusedTerminal
              ? t('quickCommands.runIn', { terminal: focusedTerminalLabel ?? t('quickCommands.focusedTerminal') })
              : t('quickCommands.runUnavailable')"
            :disabled="!canRunInFocusedTerminal || runningId !== null"
            @click="run(item)"
          >
            <NvxIcon
              :icon="Play"
              :size="16"
            />
            {{ t("quickCommands.runAction") }}
          </NvxButton>
          <NvxButton
            class="quick-command__copy"
            variant="ghost"
            size="sm"
            :aria-label="t('quickCommands.copy', { label: item.label })"
            @click="copy(item)"
          >
            <NvxIcon
              :icon="copiedId === item.id ? Check : Copy"
              :size="16"
            />
            {{ t("quickCommands.copyAction") }}
          </NvxButton>
          <NvxIconButton
            :label="t('quickCommands.moreActions')"
            size="sm"
            aria-haspopup="menu"
            :aria-expanded="menuOpen && menuItem?.id === item.id"
            @click="toggleActions(item, $event)"
          >
            <NvxIcon
              :icon="Ellipsis"
              :size="16"
            />
          </NvxIconButton>
        </div>
      </article>
    </div>

    <Teleport to="body">
      <div
        v-if="menuOpen && menuItem"
        :ref="viewportPanelRef"
        class="quick-command__menu"
        :style="viewportPanelStyle"
        role="menu"
        :aria-label="t('quickCommands.moreActions')"
        @keydown="handleMenuKeyDown"
      >
        <button
          class="quick-command__menu-item"
          type="button"
          role="menuitem"
          :aria-label="t('quickCommands.edit', { label: menuItem.label })"
          @click="openEdit(menuItem)"
        >
          <NvxIcon
            :icon="Pencil"
            :size="16"
          />
          {{ t("quickCommands.editAction") }}
        </button>
        <button
          class="quick-command__menu-item quick-command__menu-item--danger"
          type="button"
          role="menuitem"
          :aria-label="t('quickCommands.delete', { label: menuItem.label })"
          @click="requestDelete(menuItem)"
        >
          <NvxIcon
            :icon="Trash2"
            :size="16"
          />
          {{ t("quickCommands.deleteAction") }}
        </button>
      </div>
    </Teleport>

    <div
      v-if="!quickCommands.commands.length"
      class="quick-commands__empty"
    >
      <NvxIcon
        :icon="Command"
        :size="20"
      />
      <strong>{{ t("quickCommands.emptyTitle") }}</strong>
      <p>{{ t("quickCommands.emptyBody") }}</p>
      <NvxButton
        variant="secondary"
        size="sm"
        @click="openCreate"
      >
        <NvxIcon
          :icon="Plus"
          :size="16"
        />
        {{ t("quickCommands.add") }}
      </NvxButton>
    </div>

    <NvxDialog
      v-model="editorOpen"
      :title="editorTitle"
      :description="t('quickCommands.editorDescription')"
      :close-label="t('quickCommands.closeEditor')"
    >
      <NvxField
        for-id="quick-command-label"
        :label="t('quickCommands.label')"
      >
        <NvxInput
          id="quick-command-label"
          v-model="label"
          :placeholder="t('quickCommands.labelPlaceholder')"
          :maxlength="MAX_QUICK_COMMAND_LABEL_LENGTH"
          data-nvx-dialog-initial-focus
        />
      </NvxField>
      <NvxField
        for-id="quick-command-value"
        :label="t('quickCommands.command')"
        :hint="t('quickCommands.commandHint')"
      >
        <NvxTextarea
          id="quick-command-value"
          v-model="command"
          :placeholder="t('quickCommands.commandPlaceholder')"
          :maxlength="MAX_QUICK_COMMAND_LENGTH"
        />
      </NvxField>
      <NvxInlineNotice
        v-if="saveErrorKey"
        tone="error"
        :title="t(saveErrorKey)"
      />
      <template #actions>
        <NvxButton
          variant="ghost"
          @click="editorOpen = false"
        >
          {{ t("quickCommands.cancel") }}
        </NvxButton>
        <NvxButton @click="save">
          {{ t("quickCommands.save") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      v-model="deleteOpen"
      :title="t('quickCommands.deleteTitle')"
      :description="t('quickCommands.deleteDescription', { label: pendingDelete?.label ?? '' })"
      :close-label="t('quickCommands.closeDelete')"
    >
      <NvxInlineNotice
        v-if="deleteErrorVisible"
        tone="error"
        :title="t('quickCommands.errors.storage-error')"
      />
      <template #actions>
        <NvxButton
          variant="ghost"
          @click="deleteOpen = false"
        >
          {{ t("quickCommands.cancel") }}
        </NvxButton>
        <NvxButton
          variant="danger"
          @click="confirmDelete"
        >
          {{ t("quickCommands.confirmDelete") }}
        </NvxButton>
      </template>
    </NvxDialog>
  </aside>
</template>

<style scoped>
.quick-commands {
  display: flex;
  flex: 0 0 clamp(var(--nvx-layout-inspector-width), 30vw, 380px);
  min-width: 0;
  min-height: 0;
  flex-direction: column;
  width: clamp(var(--nvx-layout-inspector-width), 30vw, 380px);
  border-left: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
}

@media (max-width: 1100px) {
  .quick-commands {
    position: absolute;
    z-index: var(--nvx-z-popover);
    top: 0;
    right: 0;
    bottom: 0;
  }
}

.quick-commands__header {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  justify-content: space-between;
  min-height: 56px;
  box-sizing: border-box;
  padding: 0 var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.quick-commands h2,
.quick-commands p {
  margin: 0;
}

.quick-commands h2 {
  font-size: var(--nvx-font-size-body);
}

.quick-commands__feedback,
.quick-commands__empty p {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
}

.quick-commands__feedback {
  padding: var(--nvx-space-2) var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.quick-commands__empty p {
  margin-top: var(--nvx-space-1);
}

.quick-commands__list {
  min-height: 0;
  overflow: auto;
}

.quick-command {
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: var(--nvx-space-2);
  padding: var(--nvx-space-3) var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.quick-command__content {
  display: grid;
  flex: 1 1 auto;
  min-width: 0;
  gap: 2px;
}

.quick-command:hover {
  background: var(--nvx-color-bg-subtle);
}

.quick-command__content strong,
.quick-command code {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.quick-command code {
  display: block;
  color: var(--nvx-color-text-secondary);
  font-family: var(--nvx-font-mono);
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
}

.quick-command__actions {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 2px;
}

.quick-command__actions .nvx-button {
  padding-inline: var(--nvx-space-1);
}

.quick-command__actions .nvx-button :deep(.nvx-button__content) {
  gap: var(--nvx-space-1);
}

.quick-command__run:not(:disabled) {
  color: var(--nvx-color-accent);
}

.quick-command__menu {
  position: fixed;
  z-index: var(--nvx-z-popover);
  display: grid;
  box-sizing: border-box;
  width: min(168px, calc(100vw - 24px));
  padding: 2px;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-overlay);
}

.quick-command__menu-item {
  display: flex;
  min-height: 32px;
  align-items: center;
  gap: var(--nvx-space-2);
  padding: 0 var(--nvx-space-2);
  border: 0;
  border-radius: var(--nvx-radius-sm);
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  font-size: var(--nvx-font-size-sm);
  text-align: left;
  cursor: pointer;
}

.quick-command__menu-item:hover,
.quick-command__menu-item:focus-visible {
  outline: none;
  background: var(--nvx-color-bg-hover);
}

.quick-command__menu-item--danger {
  color: var(--nvx-color-danger);
}

.quick-commands__empty {
  display: grid;
  justify-items: center;
  margin: auto;
  padding: var(--nvx-space-6) var(--nvx-space-4);
  color: var(--nvx-color-text-secondary);
  text-align: center;
}

.quick-commands__empty strong {
  margin-top: var(--nvx-space-3);
  color: var(--nvx-color-text-primary);
}

.quick-commands__empty .nvx-button {
  margin-top: var(--nvx-space-4);
}
</style>
