<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import {
  clearNativeTerminalHistory,
  getNativeTerminalSettings,
  pauseNativeTerminalHistory,
  replaceNativeTerminalSettings,
} from "../../core-api/native-terminal";
import { useTipsStore } from "../../stores/tips";
import type { NativeTerminalSettings as NativeShellSettings, NativeTerminalSettingsSnapshot as NativeShellSettingsSnapshot } from "../../core-api/generated/core-api";
import {
  NvxButton,
  NvxCheckbox,
  NvxDialog,
  NvxField,
  NvxInlineNotice,
  NvxInput,
} from "../ui";

const emit = defineEmits<{ saved: [] }>();
defineSlots<{ "notification-permission"(): unknown }>();

const { t } = useI18n();
const tips = useTipsStore();
const loading = ref(true);
const loadError = ref(false);
const saving = ref(false);
const pauseSaving = ref(false);
const clearing = ref(false);
const clearDialogOpen = ref(false);
const snapshot = ref<NativeShellSettingsSnapshot | null>(null);
const settingsRevision = ref<string | null>(null);
const baseline = ref("");
const draft = ref<NativeShellSettings>({
  historyEnabled: false,
  persistEncrypted: false,
  historyMaxEntries: 500,
  historyRetentionDays: 30,
  historyPaused: false,
  notificationsEnabled: false,
  notificationThresholdSeconds: 60,
});
const thresholdDraft = ref("60");
const maxEntriesDraft = ref("500");
const retentionDaysDraft = ref("30");

function cloneSettings(settings: NativeShellSettings): NativeShellSettings {
  return { ...settings };
}

function parseBounded(value: string, minimum: number, maximum: number) {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed >= minimum && parsed <= maximum ? parsed : null;
}

const thresholdValid = computed(() => parseBounded(thresholdDraft.value, 1, 3_600) !== null);
const maxEntriesValid = computed(() => parseBounded(maxEntriesDraft.value, 100, 2_000) !== null);
const retentionDaysValid = computed(() => parseBounded(retentionDaysDraft.value, 1, 90) !== null);
const numericValid = computed(() => thresholdValid.value && maxEntriesValid.value && retentionDaysValid.value);

function settingsFromDraft(): NativeShellSettings | null {
  const notificationThresholdSeconds = parseBounded(thresholdDraft.value, 1, 3_600);
  const historyMaxEntries = parseBounded(maxEntriesDraft.value, 100, 2_000);
  const historyRetentionDays = parseBounded(retentionDaysDraft.value, 1, 90);
  if (notificationThresholdSeconds === null || historyMaxEntries === null || historyRetentionDays === null) {
    return null;
  }
  return {
    ...draft.value,
    notificationThresholdSeconds,
    historyMaxEntries,
    historyRetentionDays,
  };
}

function draftSignature() {
  return JSON.stringify(settingsFromDraft() ?? {
    ...draft.value,
    notificationThresholdSeconds: thresholdDraft.value,
    historyMaxEntries: maxEntriesDraft.value,
    historyRetentionDays: retentionDaysDraft.value,
  });
}

const dirty = computed(() => draftSignature() !== baseline.value);
const historyStatus = computed<"available" | "disabled" | "vaultLocked" | "persistenceFailed" | "unavailable">(() => {
  const current = snapshot.value;
  if (!current) return "unavailable";
  if (current.historyPersistenceFailed) return "persistenceFailed";
  if (current.settings.persistEncrypted && !current.historyAvailable) return "vaultLocked";
  if (!current.settings.historyEnabled) return "disabled";
  return current.historyAvailable ? "available" : "unavailable";
});

function applyDraft(settings: NativeShellSettings) {
  draft.value = cloneSettings(settings);
  thresholdDraft.value = String(settings.notificationThresholdSeconds);
  maxEntriesDraft.value = String(settings.historyMaxEntries);
  retentionDaysDraft.value = String(settings.historyRetentionDays);
}

function setServerSnapshot(next: NativeShellSettingsSnapshot) {
  snapshot.value = next;
  settingsRevision.value = next.settingsRevision;
  baseline.value = JSON.stringify(next.settings);
}

function applyServerSnapshot(next: NativeShellSettingsSnapshot) {
  setServerSnapshot(next);
  applyDraft(next.settings);
}

async function load() {
  loading.value = true;
  loadError.value = false;
  try {
    const result = await getNativeTerminalSettings() as NativeShellSettingsSnapshot;
    if (!snapshot.value || !dirty.value) applyServerSnapshot(result);
    else setServerSnapshot(result);
  } catch {
    loadError.value = true;
  } finally {
    loading.value = false;
  }
}

function discardDraft() {
  if (snapshot.value) applyServerSnapshot(snapshot.value);
}

async function save() {
  const submitted = settingsFromDraft();
  const revision = settingsRevision.value;
  if (!submitted || !revision || saving.value) return;
  const submittedSignature = draftSignature();
  saving.value = true;
  try {
    const result = await replaceNativeTerminalSettings(submitted, revision) as NativeShellSettingsSnapshot;
    setServerSnapshot(result);
    if (draftSignature() === submittedSignature) applyDraft(result.settings);
    emit("saved");
    tips.show({ scope: "native-shell-settings", tone: "success", title: t("nativeShellSettings.saved") });
  } catch {
    tips.show({ scope: "native-shell-settings", tone: "error", title: t("nativeShellSettings.saveFailed") });
  } finally {
    saving.value = false;
  }
}

async function togglePauseNow() {
  const current = snapshot.value;
  if (!current || pauseSaving.value) return;
  const paused = !current.settings.historyPaused;
  const submittedSignature = draftSignature();
  pauseSaving.value = true;
  try {
    const result = await pauseNativeTerminalHistory(paused) as NativeShellSettingsSnapshot;
    setServerSnapshot(result);
    if (draftSignature() === submittedSignature) applyDraft(result.settings);
    else if (draft.value.historyPaused === paused) draft.value.historyPaused = result.settings.historyPaused;
    emit("saved");
    tips.show({
      scope: "native-shell-settings",
      tone: "success",
      title: t(paused ? "nativeShellSettings.paused" : "nativeShellSettings.resumed"),
    });
  } catch {
    tips.show({ scope: "native-shell-settings", tone: "error", title: t("nativeShellSettings.pauseFailed") });
  } finally {
    pauseSaving.value = false;
  }
}

async function clearHistory() {
  if (clearing.value) return;
  clearing.value = true;
  try {
    await clearNativeTerminalHistory(null);
    clearDialogOpen.value = false;
    tips.show({ scope: "native-shell-settings", tone: "success", title: t("nativeShellSettings.cleared") });
  } catch {
    tips.show({ scope: "native-shell-settings", tone: "error", title: t("nativeShellSettings.clearFailed") });
  } finally {
    clearing.value = false;
  }
}

onMounted(() => void load());
</script>

<template>
  <section
    class="nvx-native-shell-settings"
    :aria-label="t('nativeShellSettings.title')"
  >
    <header class="nvx-native-shell-settings__header">
      <div>
        <h2>{{ t("nativeShellSettings.title") }}</h2>
        <p>{{ t("nativeShellSettings.description") }}</p>
      </div>
    </header>

    <NvxInlineNotice
      v-if="loading"
      tone="info"
    >
      {{ t("nativeShellSettings.loading") }}
    </NvxInlineNotice>
    <NvxInlineNotice
      v-else-if="loadError"
      tone="error"
      :title="t('nativeShellSettings.loadFailed')"
    >
      <NvxButton
        variant="ghost"
        size="sm"
        @click="load"
      >
        {{ t("nativeShellSettings.retry") }}
      </NvxButton>
    </NvxInlineNotice>

    <template v-else-if="snapshot">
      <NvxInlineNotice
        tone="info"
        :title="t('nativeShellSettings.shellNoticeTitle')"
      >
        {{ t("nativeShellSettings.shellNotice") }}
      </NvxInlineNotice>

      <section class="nvx-native-shell-settings__section">
        <h3>{{ t("nativeShellSettings.notifications.title") }}</h3>
        <div class="nvx-native-shell-settings__rows">
          <NvxCheckbox v-model="draft.notificationsEnabled">
            {{ t("nativeShellSettings.notifications.enabled") }}
          </NvxCheckbox>
          <NvxField
            class="nvx-native-shell-settings__field"
            :label="t('nativeShellSettings.notifications.threshold')"
            :hint="t('nativeShellSettings.notifications.thresholdHint')"
            :error="thresholdValid ? undefined : t('nativeShellSettings.invalid')"
          >
            <NvxInput
              :model-value="thresholdDraft"
              type="number"
              :min="1"
              :max="3600"
              :invalid="!thresholdValid"
              @update:model-value="thresholdDraft = $event"
            />
          </NvxField>
          <div class="nvx-native-shell-settings__permission">
            <strong>{{ t("nativeShellSettings.notifications.permission") }}</strong>
            <slot name="notification-permission" />
          </div>
        </div>
      </section>

      <section class="nvx-native-shell-settings__section">
        <div class="nvx-native-shell-settings__section-heading">
          <h3>{{ t("nativeShellSettings.history.title") }}</h3>
          <span class="nvx-native-shell-settings__history-status">
            {{ t(`nativeShellSettings.history.${historyStatus}`) }}
          </span>
        </div>
        <NvxInlineNotice
          :tone="historyStatus === 'vaultLocked' || historyStatus === 'persistenceFailed' || historyStatus === 'unavailable' ? 'warning' : 'info'"
        >
          {{ t(`nativeShellSettings.history.${historyStatus}`) }}
        </NvxInlineNotice>
        <div class="nvx-native-shell-settings__rows">
          <NvxCheckbox v-model="draft.historyEnabled">
            {{ t("nativeShellSettings.history.enabled") }}
            <template #hint>
              {{ t("nativeShellSettings.history.enabledHint") }}
            </template>
          </NvxCheckbox>
          <NvxCheckbox v-model="draft.historyPaused">
            {{ t("nativeShellSettings.history.pause") }}
            <template #hint>
              {{ t("nativeShellSettings.history.pauseHint") }}
            </template>
          </NvxCheckbox>
          <NvxCheckbox v-model="draft.persistEncrypted">
            {{ t("nativeShellSettings.history.persist") }}
            <template #hint>
              {{ t("nativeShellSettings.history.persistHint") }}
            </template>
          </NvxCheckbox>
          <div class="nvx-native-shell-settings__number-grid">
            <NvxField
              :label="t('nativeShellSettings.history.maxEntries')"
              :hint="t('nativeShellSettings.history.maxEntriesHint')"
              :error="maxEntriesValid ? undefined : t('nativeShellSettings.invalid')"
            >
              <NvxInput
                :model-value="maxEntriesDraft"
                type="number"
                :min="100"
                :max="2000"
                :invalid="!maxEntriesValid"
                @update:model-value="maxEntriesDraft = $event"
              />
            </NvxField>
            <NvxField
              :label="t('nativeShellSettings.history.retentionDays')"
              :hint="t('nativeShellSettings.history.retentionHint')"
              :error="retentionDaysValid ? undefined : t('nativeShellSettings.invalid')"
            >
              <NvxInput
                :model-value="retentionDaysDraft"
                type="number"
                :min="1"
                :max="90"
                :invalid="!retentionDaysValid"
                @update:model-value="retentionDaysDraft = $event"
              />
            </NvxField>
          </div>
        </div>
        <div class="nvx-native-shell-settings__actions">
          <NvxButton
            variant="ghost"
            size="sm"
            :loading="pauseSaving"
            :disabled="!snapshot.historyAvailable || !snapshot.settings.historyEnabled"
            @click="togglePauseNow"
          >
            {{ snapshot.settings.historyPaused
              ? t("nativeShellSettings.history.resumeNow")
              : t("nativeShellSettings.history.pauseNow") }}
          </NvxButton>
          <NvxButton
            variant="danger"
            size="sm"
            :disabled="!snapshot.historyAvailable"
            @click="clearDialogOpen = true"
          >
            {{ t("nativeShellSettings.history.clear") }}
          </NvxButton>
        </div>
      </section>

      <NvxInlineNotice
        tone="warning"
        :title="t('nativeShellSettings.privacyNoticeTitle')"
      >
        {{ t("nativeShellSettings.privacyNotice") }}
      </NvxInlineNotice>

      <footer class="nvx-native-shell-settings__footer">
        <span v-if="dirty">{{ t("nativeShellSettings.unsaved") }}</span>
        <div class="nvx-native-shell-settings__actions">
          <NvxButton
            variant="ghost"
            :disabled="!dirty || saving"
            @click="discardDraft"
          >
            {{ t("nativeShellSettings.discard") }}
          </NvxButton>
          <NvxButton
            :loading="saving"
            :disabled="!dirty || !numericValid"
            @click="save"
          >
            {{ t("nativeShellSettings.save") }}
          </NvxButton>
        </div>
      </footer>
    </template>

    <NvxDialog
      v-model="clearDialogOpen"
      :title="t('nativeShellSettings.history.clearDialogTitle')"
      :description="t('nativeShellSettings.history.clearDialogDescription')"
      :close-label="t('nativeShellSettings.history.clearCancel')"
      :dismissible="!clearing"
    >
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="clearing"
          @click="clearDialogOpen = false"
        >
          {{ t("nativeShellSettings.history.clearCancel") }}
        </NvxButton>
        <NvxButton
          variant="danger"
          :loading="clearing"
          @click="clearHistory"
        >
          {{ t("nativeShellSettings.history.clearConfirm") }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.nvx-native-shell-settings { display: grid; gap: var(--nvx-space-4); min-width: 0; }
.nvx-native-shell-settings__header h2, .nvx-native-shell-settings__section h3 { margin: 0; color: var(--nvx-color-text-primary); }
.nvx-native-shell-settings__header h2 { font-size: var(--nvx-font-size-lg); line-height: var(--nvx-line-height-lg); }
.nvx-native-shell-settings__header p { max-width: 780px; margin: var(--nvx-space-1) 0 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); line-height: var(--nvx-line-height-body); }
.nvx-native-shell-settings__section { display: grid; gap: var(--nvx-space-3); padding-block: var(--nvx-space-3); border-top: var(--nvx-border-width) solid var(--nvx-color-border); }
.nvx-native-shell-settings__section h3 { font-size: var(--nvx-font-size-sm); line-height: var(--nvx-line-height-sm); }
.nvx-native-shell-settings__section-heading { display: flex; gap: var(--nvx-space-3); align-items: baseline; justify-content: space-between; }
.nvx-native-shell-settings__history-status { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); text-align: end; }
.nvx-native-shell-settings__rows { display: grid; gap: var(--nvx-space-2); }
.nvx-native-shell-settings__field { max-width: 330px; }
.nvx-native-shell-settings__permission { display: grid; gap: var(--nvx-space-2); padding: var(--nvx-space-3); border: var(--nvx-border-width) solid var(--nvx-color-border-subtle); border-radius: var(--nvx-radius-md); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.nvx-native-shell-settings__permission strong { color: var(--nvx-color-text-primary); font-weight: var(--nvx-font-weight-medium); }
.nvx-native-shell-settings__number-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--nvx-space-3); max-width: 680px; }
.nvx-native-shell-settings__actions, .nvx-native-shell-settings__footer { display: flex; flex-wrap: wrap; gap: var(--nvx-space-2); align-items: center; }
.nvx-native-shell-settings__actions { justify-content: flex-end; }
.nvx-native-shell-settings__footer { justify-content: space-between; padding-top: var(--nvx-space-3); border-top: var(--nvx-border-width) solid var(--nvx-color-border); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
@media (max-width: 680px) { .nvx-native-shell-settings__number-grid { grid-template-columns: 1fr; } .nvx-native-shell-settings__footer { align-items: flex-start; flex-direction: column; } .nvx-native-shell-settings__actions { justify-content: flex-start; } }
</style>
