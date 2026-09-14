<script setup lang="ts">
import { Ban, Download, Keyboard, RotateCcw, Upload } from "lucide-vue-next";
import { computed, onBeforeUnmount, ref, useTemplateRef } from "vue";
import { useI18n } from "vue-i18n";

import {
  formatShortcutBinding,
  shortcutFromKeyboardEvent,
  type ShortcutBinding,
  type ShortcutCommand,
  type ShortcutCommandId,
  type ShortcutPlatform,
} from "../../shortcuts";
import { detectDesktopPlatform } from "../../platform";
import { exportJsonFile } from "../../platform-file-export";
import { useShortcutsStore, type ShortcutSaveResult } from "../../stores/shortcuts";
import { useTipsStore } from "../../stores/tips";
import { NvxButton, NvxDialog, NvxField, NvxIcon, NvxInlineNotice, NvxInput, NvxSelect } from "../ui";

const { t } = useI18n();
const shortcuts = useShortcutsStore();
const tips = useTipsStore();
const initialPlatform = detectDesktopPlatform() === "windows" ? "windows" : "macos";

const platform = ref<ShortcutPlatform>(initialPlatform);
const query = ref("");
const recordingCommand = ref<ShortcutCommand | null>(null);
const recordingBinding = ref<ShortcutBinding>(null);
const recordingError = ref("");
const resetDialogOpen = ref(false);
const importInput = useTemplateRef<HTMLInputElement>("importInput");
const exporting = ref(false);
let alive = true;

const platformOptions = computed(() => [
  { value: "macos", label: t("shortcuts.platforms.macos") },
  { value: "windows", label: t("shortcuts.platforms.windows") },
]);

const groupedCommands = computed(() => {
  const normalized = query.value.trim().toLocaleLowerCase();
  const groups = new Map<string, ShortcutCommand[]>();
  for (const item of shortcuts.commands) {
    const title = t(`shortcuts.commands.${item.messageKey}.title`);
    const description = t(`shortcuts.commands.${item.messageKey}.description`);
    const category = t(`shortcuts.categories.${item.category}`);
    if (normalized && ![title, description, category].some((value) => value.toLocaleLowerCase().includes(normalized))) {
      continue;
    }
    const existing = groups.get(item.category) ?? [];
    existing.push(item);
    groups.set(item.category, existing);
  }
  return [...groups.entries()].map(([category, items]) => ({ category, items }));
});

const selectedBindings = computed(() => shortcuts.bindingsFor(platform.value));
const recordingLabel = computed(() => (
  recordingBinding.value ? formatShortcutBinding(recordingBinding.value, platform.value) : t("shortcuts.disabled")
));

function commandTitle(commandId: ShortcutCommandId) {
  const command = shortcuts.commands.find((item) => item.id === commandId);
  return command ? t(`shortcuts.commands.${command.messageKey}.title`) : commandId;
}

function validationMessage(reason: string, conflicts?: ShortcutCommandId[]) {
  if (reason === "conflict") {
    return t("shortcuts.errors.conflict", { commands: (conflicts ?? []).map(commandTitle).join("、") });
  }
  const keys: Record<string, string> = {
    invalid: "shortcuts.errors.invalid",
    "missing-modifier": "shortcuts.errors.missingModifier",
    "platform-modifier": "shortcuts.errors.platformModifier",
    "system-reserved": "shortcuts.errors.systemReserved",
    "storage-error": "shortcuts.errors.storageError",
  };
  return t(keys[reason] ?? "shortcuts.errors.invalid");
}

function showSaveResult(result: ShortcutSaveResult, statusKey: "saved" | "disabled" | "reset") {
  if (result.ok) {
    tips.show({ scope: "shortcuts", tone: "success", title: t(`shortcuts.status.${statusKey}`) });
    return true;
  }
  recordingError.value = validationMessage(result.reason, result.conflicts);
  if (result.reason === "storage-error") {
    tips.show({ scope: "shortcuts", tone: "error", title: recordingError.value });
  }
  return false;
}

function openRecorder(command: ShortcutCommand) {
  recordingCommand.value = command;
  recordingBinding.value = selectedBindings.value[command.id];
  recordingError.value = "";
}

function closeRecorder() {
  recordingCommand.value = null;
  recordingBinding.value = null;
  recordingError.value = "";
}

function captureShortcut(event: KeyboardEvent) {
  if (!recordingCommand.value) return;
  event.preventDefault();
  event.stopPropagation();
  if (event.key === "Escape") {
    closeRecorder();
    return;
  }
  const result = shortcutFromKeyboardEvent(event, platform.value, recordingCommand.value.scope);
  if (result.error) {
    recordingError.value = validationMessage(result.error);
    return;
  }
  recordingBinding.value = result.binding;
  recordingError.value = "";
}

function saveRecording() {
  const command = recordingCommand.value;
  if (!command) return;
  const result = shortcuts.setBinding(platform.value, command.id, recordingBinding.value);
  if (showSaveResult(result, "saved")) closeRecorder();
}

function disable(command: ShortcutCommand) {
  recordingError.value = "";
  showSaveResult(shortcuts.setBinding(platform.value, command.id, null), "disabled");
}

function reset(command: ShortcutCommand) {
  recordingError.value = "";
  showSaveResult(shortcuts.resetBinding(platform.value, command.id), "reset");
}

function confirmResetAll() {
  const result = shortcuts.resetAll(platform.value);
  if (showSaveResult(result, "reset")) {
    resetDialogOpen.value = false;
    tips.show({ scope: "shortcuts", tone: "success", title: t("shortcuts.status.resetAll") });
  }
}

async function exportProfile() {
  if (exporting.value) return;
  exporting.value = true;
  try {
    if (!await exportJsonFile("shortcuts", shortcuts.exportProfile())) return;
    if (alive) tips.show({ scope: "shortcuts", tone: "success", title: t("shortcuts.status.exported") });
  } catch {
    if (alive) tips.show({ scope: "shortcuts", tone: "error", title: t("shortcuts.errors.exportWrite") });
  } finally {
    if (alive) exporting.value = false;
  }
}

function importErrorMessage(reason: string) {
  const keys: Record<string, string> = {
    "invalid-json": "shortcuts.errors.importInvalidJson",
    "invalid-profile": "shortcuts.errors.importInvalidProfile",
    "too-large": "shortcuts.errors.importTooLarge",
    conflict: "shortcuts.errors.importConflict",
    "storage-error": "shortcuts.errors.storageError",
  };
  return t(keys[reason] ?? "shortcuts.errors.importInvalidProfile");
}

async function importFile(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  try {
    const result = shortcuts.importProfile(await file.text());
    if (result.ok) {
      tips.show({ scope: "shortcuts", tone: "success", title: t("shortcuts.status.imported") });
      return;
    }
    tips.show({ scope: "shortcuts", tone: "error", title: importErrorMessage(result.reason) });
  } catch {
    tips.show({ scope: "shortcuts", tone: "error", title: t("shortcuts.errors.importRead") });
  }
}

onBeforeUnmount(() => {
  alive = false;
});
</script>

<template>
  <section
    class="nvx-shortcut-settings"
    :aria-label="t('shortcuts.title')"
  >
    <header class="nvx-shortcut-settings__header">
      <div>
        <h2>{{ t("shortcuts.title") }}</h2>
        <p>{{ t("shortcuts.description") }}</p>
      </div>
      <div class="nvx-shortcut-settings__actions">
        <NvxButton
          variant="ghost"
          size="sm"
          :disabled="exporting"
          @click="exportProfile"
        >
          <NvxIcon
            :icon="Download"
            :size="16"
          />
          {{ t("shortcuts.export") }}
        </NvxButton>
        <NvxButton
          variant="ghost"
          size="sm"
          @click="importInput?.click()"
        >
          <NvxIcon
            :icon="Upload"
            :size="16"
          />
          {{ t("shortcuts.import") }}
        </NvxButton>
        <input
          ref="importInput"
          class="nvx-shortcut-settings__file-input"
          type="file"
          accept="application/json,.json"
          @change="importFile"
        >
      </div>
    </header>

    <div class="nvx-shortcut-settings__filters">
      <NvxField :label="t('shortcuts.platform')">
        <NvxSelect
          v-model="platform"
          :options="platformOptions"
        />
      </NvxField>
      <NvxField :label="t('shortcuts.search')">
        <NvxInput
          v-model="query"
          :placeholder="t('shortcuts.searchPlaceholder')"
          :aria-label="t('shortcuts.search')"
        />
      </NvxField>
      <NvxButton
        class="nvx-shortcut-settings__reset-all"
        variant="secondary"
        size="sm"
        @click="resetDialogOpen = true"
      >
        <NvxIcon
          :icon="RotateCcw"
          :size="16"
        />
        {{ t("shortcuts.resetAll") }}
      </NvxButton>
    </div>

    <NvxInlineNotice tone="info">
      {{ t("shortcuts.record.description") }}
    </NvxInlineNotice>

    <div
      v-if="groupedCommands.length"
      class="nvx-shortcut-settings__groups"
    >
      <section
        v-for="group in groupedCommands"
        :key="group.category"
        class="nvx-shortcut-settings__group"
      >
        <h3>{{ t(`shortcuts.categories.${group.category}`) }}</h3>
        <div class="nvx-shortcut-settings__rows">
          <article
            v-for="command in group.items"
            :key="command.id"
            class="nvx-shortcut-settings__row"
          >
            <div class="nvx-shortcut-settings__identity">
              <strong>{{ t(`shortcuts.commands.${command.messageKey}.title`) }}</strong>
              <span>{{ t(`shortcuts.commands.${command.messageKey}.description`) }}</span>
            </div>
            <span class="nvx-shortcut-settings__scope">{{ t(`shortcuts.scope.${command.scope}`) }}</span>
            <kbd
              v-if="selectedBindings[command.id]"
              class="nvx-shortcut-settings__binding"
            >{{ formatShortcutBinding(selectedBindings[command.id], platform) }}</kbd>
            <span
              v-else
              class="nvx-shortcut-settings__disabled"
            >{{ t("shortcuts.disabled") }}</span>
            <div class="nvx-shortcut-settings__row-actions">
              <NvxButton
                variant="ghost"
                size="sm"
                @click="openRecorder(command)"
              >
                <NvxIcon
                  :icon="Keyboard"
                  :size="16"
                />
                {{ t("shortcuts.edit") }}
              </NvxButton>
              <NvxButton
                variant="ghost"
                size="sm"
                :disabled="!selectedBindings[command.id]"
                @click="disable(command)"
              >
                <NvxIcon
                  :icon="Ban"
                  :size="16"
                />
                {{ t("shortcuts.disable") }}
              </NvxButton>
              <NvxButton
                variant="ghost"
                size="sm"
                @click="reset(command)"
              >
                {{ t("shortcuts.reset") }}
              </NvxButton>
            </div>
          </article>
        </div>
      </section>
    </div>
    <p
      v-else
      class="nvx-shortcut-settings__empty"
    >
      {{ t("shortcuts.empty") }}
    </p>

    <NvxDialog
      :model-value="Boolean(recordingCommand)"
      :title="t('shortcuts.record.title')"
      :description="t('shortcuts.record.description')"
      :close-label="t('shortcuts.record.cancel')"
      @update:model-value="!$event && closeRecorder()"
    >
      <div
        class="nvx-shortcut-settings__recorder"
        tabindex="0"
        data-nvx-dialog-initial-focus
        @keydown.capture="captureShortcut"
      >
        <strong>{{ recordingCommand ? t(`shortcuts.commands.${recordingCommand.messageKey}.title`) : "" }}</strong>
        <span>{{ t("shortcuts.record.listening") }}</span>
        <kbd>{{ recordingLabel }}</kbd>
      </div>
      <NvxInlineNotice
        v-if="recordingError"
        tone="error"
      >
        {{ recordingError }}
      </NvxInlineNotice>
      <template #actions>
        <NvxButton
          variant="ghost"
          @click="closeRecorder"
        >
          {{ t("shortcuts.record.cancel") }}
        </NvxButton>
        <NvxButton
          :disabled="!recordingBinding"
          @click="saveRecording"
        >
          {{ t("shortcuts.record.save") }}
        </NvxButton>
      </template>
    </NvxDialog>

    <NvxDialog
      v-model="resetDialogOpen"
      :title="t('shortcuts.resetDialog.title')"
      :description="t('shortcuts.resetDialog.description')"
      :close-label="t('shortcuts.resetDialog.cancel')"
    >
      <template #actions>
        <NvxButton
          variant="ghost"
          @click="resetDialogOpen = false"
        >
          {{ t("shortcuts.resetDialog.cancel") }}
        </NvxButton>
        <NvxButton
          variant="danger"
          @click="confirmResetAll"
        >
          {{ t("shortcuts.resetDialog.confirm") }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.nvx-shortcut-settings { display: grid; gap: var(--nvx-space-4); min-width: 0; }
.nvx-shortcut-settings__header { display: flex; gap: var(--nvx-space-4); align-items: flex-start; justify-content: space-between; }
.nvx-shortcut-settings__header h2, .nvx-shortcut-settings__group h3 { margin: 0; color: var(--nvx-color-text-primary); }
.nvx-shortcut-settings__header h2 { font-size: var(--nvx-font-size-lg); line-height: var(--nvx-line-height-lg); }
.nvx-shortcut-settings__header p { max-width: 780px; margin: var(--nvx-space-1) 0 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); line-height: var(--nvx-line-height-body); }
.nvx-shortcut-settings__actions, .nvx-shortcut-settings__row-actions { display: flex; flex-wrap: wrap; gap: var(--nvx-space-1); align-items: center; }
.nvx-shortcut-settings__file-input { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; clip-path: inset(50%); }
.nvx-shortcut-settings__filters { display: grid; grid-template-columns: minmax(140px, 190px) minmax(220px, 1fr) auto; gap: var(--nvx-space-3); align-items: end; padding: var(--nvx-space-3); border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-subtle); }
.nvx-shortcut-settings__reset-all { align-self: end; }
.nvx-shortcut-settings__groups { display: grid; gap: var(--nvx-space-5); }
.nvx-shortcut-settings__group { display: grid; gap: var(--nvx-space-2); }
.nvx-shortcut-settings__group h3 { font-size: var(--nvx-font-size-sm); line-height: var(--nvx-line-height-sm); }
.nvx-shortcut-settings__rows { border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); overflow: hidden; }
.nvx-shortcut-settings__row { display: grid; grid-template-columns: minmax(190px, 1fr) auto minmax(105px, auto) auto; gap: var(--nvx-space-3); align-items: center; padding: var(--nvx-space-2) var(--nvx-space-3); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); background: var(--nvx-color-bg-surface); }
.nvx-shortcut-settings__row:last-child { border-bottom: 0; }
.nvx-shortcut-settings__identity { display: grid; min-width: 0; gap: 2px; }
.nvx-shortcut-settings__identity strong { color: var(--nvx-color-text-primary); font-size: var(--nvx-font-size-sm); font-weight: var(--nvx-font-weight-medium); }
.nvx-shortcut-settings__identity span { overflow: hidden; color: var(--nvx-color-text-tertiary); font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-xs); text-overflow: ellipsis; white-space: nowrap; }
.nvx-shortcut-settings__scope, .nvx-shortcut-settings__disabled { color: var(--nvx-color-text-tertiary); font-size: var(--nvx-font-size-xs); white-space: nowrap; }
.nvx-shortcut-settings__scope { padding: 2px var(--nvx-space-2); border-radius: var(--nvx-radius-sm); background: var(--nvx-color-bg-subtle); }
.nvx-shortcut-settings__binding, .nvx-shortcut-settings__recorder kbd { display: inline-flex; justify-content: center; min-width: 86px; padding: 3px var(--nvx-space-2); border: var(--nvx-border-width) solid var(--nvx-color-border-strong); border-radius: var(--nvx-radius-sm); background: var(--nvx-color-bg-subtle); color: var(--nvx-color-text-primary); font-family: var(--nvx-font-family-mono); font-size: var(--nvx-font-size-xs); white-space: nowrap; }
.nvx-shortcut-settings__empty { margin: 0; padding: var(--nvx-space-5); color: var(--nvx-color-text-secondary); text-align: center; }
.nvx-shortcut-settings__recorder { display: grid; gap: var(--nvx-space-3); justify-items: center; padding: var(--nvx-space-5); border: var(--nvx-border-width) dashed var(--nvx-color-border-strong); border-radius: var(--nvx-radius-md); color: var(--nvx-color-text-secondary); text-align: center; }
.nvx-shortcut-settings__recorder:focus-visible { outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring); outline-offset: var(--nvx-space-1); }
.nvx-shortcut-settings__recorder strong { color: var(--nvx-color-text-primary); }
.nvx-shortcut-settings__recorder kbd { min-width: 132px; font-size: var(--nvx-font-size-md); }
@media (max-width: 880px) { .nvx-shortcut-settings__header { display: grid; } .nvx-shortcut-settings__filters, .nvx-shortcut-settings__row { grid-template-columns: 1fr; } .nvx-shortcut-settings__row-actions { justify-content: flex-start; } .nvx-shortcut-settings__identity span { white-space: normal; } }
</style>
