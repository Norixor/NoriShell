<script setup lang="ts">
import {
  Check,
  Command,
  Copy,
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
let actionMessageTimer: number | null = null;

const editorTitle = computed(() =>
  editingId.value
    ? t("quickCommands.editTitle")
    : t("quickCommands.addTitle"),
);

function openCreate() {
  editingId.value = null;
  label.value = "";
  command.value = "";
  saveErrorKey.value = null;
  editorOpen.value = true;
}

function openEdit(item: QuickCommand) {
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
  pendingDelete.value = item;
  deleteErrorVisible.value = false;
  deleteOpen.value = true;
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
    class="quick-commands"
    :aria-label="t('quickCommands.title')"
  >
    <header class="quick-commands__header">
      <div>
        <h2>{{ t("quickCommands.title") }}</h2>
        <p aria-live="polite">
          {{ actionMessage || t("quickCommands.description") }}
        </p>
      </div>
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
    <div
      v-if="quickCommands.commands.length"
      class="quick-commands__list"
    >
      <article
        v-for="item in quickCommands.commands"
        :key="item.id"
        class="quick-command"
      >
        <div class="quick-command__summary">
          <strong :title="item.label">{{ item.label }}</strong>
          <div class="quick-command__primary-actions">
            <NvxIconButton
              class="quick-command__run"
              :label="t('quickCommands.run', { label: item.label })"
              :title="canRunInFocusedTerminal
                ? t('quickCommands.runIn', { terminal: focusedTerminalLabel ?? t('quickCommands.focusedTerminal') })
                : t('quickCommands.runUnavailable')"
              size="sm"
              :disabled="!canRunInFocusedTerminal || runningId !== null"
              @click="run(item)"
            >
              <NvxIcon
                :icon="Play"
                :size="16"
              />
            </NvxIconButton>
            <NvxIconButton
              :label="t('quickCommands.copy', { label: item.label })"
              size="sm"
              @click="copy(item)"
            >
              <NvxIcon
                :icon="copiedId === item.id ? Check : Copy"
                :size="16"
              />
            </NvxIconButton>
          </div>
        </div>
        <div class="quick-command__detail">
          <code :title="item.command">{{ item.command }}</code>
          <div class="quick-command__manage-actions">
            <NvxIconButton
              :label="t('quickCommands.edit', { label: item.label })"
              size="sm"
              @click="openEdit(item)"
            >
              <NvxIcon
                :icon="Pencil"
                :size="16"
              />
            </NvxIconButton>
            <NvxIconButton
              :label="t('quickCommands.delete', { label: item.label })"
              size="sm"
              @click="requestDelete(item)"
            >
              <NvxIcon
                :icon="Trash2"
                :size="16"
              />
            </NvxIconButton>
          </div>
        </div>
      </article>
    </div>

    <div
      v-else
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
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
  flex-direction: column;
  width: var(--nvx-layout-inspector-width);
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
  align-items: flex-start;
  justify-content: space-between;
  padding: var(--nvx-space-3);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.quick-commands h2,
.quick-commands p {
  margin: 0;
}

.quick-commands h2 {
  font-size: var(--nvx-font-size-body);
}

.quick-commands__header p,
.quick-commands__empty p {
  margin-top: var(--nvx-space-1);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
}

.quick-commands__list {
  min-height: 0;
  overflow: auto;
}

.quick-command {
  display: grid;
  gap: 2px;
  padding: var(--nvx-space-1) var(--nvx-space-2);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.quick-command__summary,
.quick-command__detail {
  display: flex;
  min-width: 0;
  gap: var(--nvx-space-1);
  align-items: center;
  justify-content: space-between;
}

.quick-command:hover {
  background: var(--nvx-color-bg-subtle);
}

.quick-command__summary strong,
.quick-command code {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.quick-command code {
  display: block;
  flex: 1 1 auto;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
}

.quick-command__primary-actions,
.quick-command__manage-actions {
  display: flex;
  flex: 0 0 auto;
}

.quick-command__run:not(:disabled) {
  color: var(--nvx-color-accent);
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
