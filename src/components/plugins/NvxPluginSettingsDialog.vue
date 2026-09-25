<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, useId, watch } from "vue";
import { useI18n } from "vue-i18n";

import { getPluginSettings, parseCoreApiError, replacePluginSettings, resetPluginSettings } from "../../core-api/client";
import type { PluginSettingsSnapshot } from "../../core-api/generated/core-api";
import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import { useTipsStore } from "../../stores/tips";
import { NvxButton, NvxCheckbox, NvxDialog, NvxField, NvxInlineNotice, NvxInput, NvxSelect } from "../ui";

const props = defineProps<{ pluginId: string; pluginName: string; initialFieldKey?: string }>();
const emit = defineEmits<{ close: []; saved: [snapshot: PluginSettingsSnapshot] }>();
const { locale, t } = useI18n();
const extensions = usePluginExtensionsStore();
const tips = useTipsStore();
const id = useId();
const snapshot = ref<PluginSettingsSnapshot | null>(null);
const draft = ref<PluginSettingsSnapshot["values"]>({});
const loading = ref(false);
const busy = ref(false);
const errorKey = ref<string | null>(null);
const conflicted = ref(false);
let generation = 0;
let disposed = false;
const label = (text: PluginSettingsSnapshot["schema"]["fields"][number]["label"]) => (
  locale.value === "zh-CN" ? text["zh-CN"] : text.en
);
const invalid = computed(() => snapshot.value?.schema.fields.some((field) => {
  const value = draft.value[field.key];
  if (field.type === "boolean") return typeof value !== "boolean";
  if (field.type === "number") return typeof value !== "number" || !Number.isFinite(value)
    || (field.min != null && value < field.min) || (field.max != null && value > field.max);
  if (typeof value !== "string") return true;
  if (field.type === "select") return !field.options?.some((option) => option.value === value);
  return field.maxLength != null && [...value].length > field.maxLength;
}) ?? false);

async function load() {
  const current = ++generation;
  loading.value = true;
  errorKey.value = null;
  conflicted.value = false;
  snapshot.value = null;
  draft.value = {};
  try {
    const next = await getPluginSettings({ pluginId: props.pluginId });
    if (disposed || current !== generation) return;
    snapshot.value = next;
    draft.value = next ? { ...next.values } : {};
  } catch {
    if (!disposed && current === generation) errorKey.value = "plugins.settings.loadFailed";
  } finally {
    if (!disposed && current === generation) loading.value = false;
  }
}

async function save(reset = false) {
  const current = snapshot.value;
  if (!current || busy.value || conflicted.value || (!reset && invalid.value)) return;
  const requestGeneration = generation;
  busy.value = true;
  errorKey.value = null;
  const fence = {
    pluginId: current.pluginId,
    expectedPackageSha256: current.packageSha256,
    expectedInstalledStateVersion: current.installedStateVersion,
    expectedSchemaSha256: current.schemaSha256,
    expectedRevision: current.revision,
  };
  try {
    const updated = reset ? await resetPluginSettings(fence)
      : await replacePluginSettings({ ...fence, values: { ...draft.value } });
    // Other mounted tools and hidden regions share the plugin revision.
    await extensions.refreshPluginContributions(current.pluginId);
    if (disposed || requestGeneration !== generation) return;
    snapshot.value = updated;
    draft.value = { ...updated.values };
    tips.show({ scope: `plugin-settings:${current.pluginId}`, tone: "success",
      title: t(reset ? "plugins.settings.resetDone" : "plugins.settings.saved") });
    emit("saved", updated);
    if (!reset) emit("close");
  } catch (cause) {
    if (disposed || requestGeneration !== generation) return;
    const code = parseCoreApiError(cause)?.code ?? "";
    conflicted.value = /stale|conflict|revision|package_changed|schema_changed/.test(code);
    errorKey.value = conflicted.value ? "plugins.settings.conflict" : "plugins.settings.saveFailed";
  } finally {
    if (!disposed && requestGeneration === generation) busy.value = false;
  }
}

watch(() => props.pluginId, () => { busy.value = false; void load(); }, { immediate: true });
watch(snapshot, async (current) => {
  if (!current || !props.initialFieldKey
    || !current.schema.fields.some((field) => field.key === props.initialFieldKey)) return;
  await nextTick();
  if (!disposed) document.getElementById(`${id}-${props.initialFieldKey}`)?.focus();
});
onBeforeUnmount(() => { disposed = true; generation += 1; });
</script>

<template>
  <NvxDialog
    :model-value="true"
    :title="t('plugins.settings.title', { plugin: pluginName })"
    :description="t('plugins.settings.description')"
    :close-label="t('plugins.settings.close')"
    :dismissible="!busy"
    plugin-protected
    :theme-protected="false"
    @close="emit('close')"
  >
    <div
      class="plugin-settings"
      :aria-busy="loading || busy"
    >
      <NvxInlineNotice
        v-if="loading"
        tone="info"
      >
        {{ t('plugins.settings.loading') }}
      </NvxInlineNotice>
      <NvxInlineNotice
        v-else-if="!snapshot && !errorKey"
        tone="info"
      >
        {{ t('plugins.settings.unavailable') }}
      </NvxInlineNotice>
      <template v-if="snapshot">
        <template
          v-for="field in snapshot.schema.fields"
          :key="field.key"
        >
          <NvxCheckbox
            v-if="field.type === 'boolean'"
            :id="`${id}-${field.key}`"
            :model-value="draft[field.key] === true"
            :disabled="busy || conflicted"
            @update:model-value="draft[field.key] = $event"
          >
            {{ label(field.label) }}
          </NvxCheckbox>
          <NvxField
            v-else
            :for-id="`${id}-${field.key}`"
            :label="label(field.label)"
          >
            <NvxSelect
              v-if="field.type === 'select'"
              :id="`${id}-${field.key}`"
              :model-value="String(draft[field.key] ?? '')"
              :options="field.options?.map(option => ({ value: option.value, label: label(option.label) }))"
              :disabled="busy || conflicted"
              :aria-label="label(field.label)"
              @update:model-value="draft[field.key] = $event"
            />
            <NvxInput
              v-else
              :id="`${id}-${field.key}`"
              :data-nvx-dialog-initial-focus="initialFieldKey === field.key ? '' : undefined"
              :model-value="String(draft[field.key] ?? '')"
              :type="field.type === 'number' ? 'number' : 'text'"
              :min="field.type === 'number' ? field.min ?? undefined : undefined"
              :max="field.type === 'number' ? field.max ?? undefined : undefined"
              :maxlength="field.type === 'string' ? field.maxLength : undefined"
              :disabled="busy || conflicted"
              @update:model-value="draft[field.key] = field.type === 'number' && $event.trim() !== '' ? Number($event) : $event"
            />
          </NvxField>
        </template>
      </template>
      <NvxInlineNotice
        v-if="errorKey"
        tone="error"
      >
        {{ t(errorKey) }}
      </NvxInlineNotice>
      <NvxInlineNotice
        v-else-if="invalid"
        tone="warning"
      >
        {{ t('plugins.settings.invalid') }}
      </NvxInlineNotice>
      <NvxButton
        v-if="errorKey && (!snapshot || conflicted)"
        variant="ghost"
        :disabled="busy || loading"
        @click="load"
      >
        {{ t('plugins.settings.reload') }}
      </NvxButton>
    </div>
    <template #actions>
      <NvxButton
        class="plugin-settings__reset"
        variant="ghost"
        :disabled="!snapshot || loading || busy || conflicted"
        @click="save(true)"
      >
        {{ t('plugins.settings.reset') }}
      </NvxButton>
      <NvxButton
        variant="ghost"
        :disabled="busy"
        @click="emit('close')"
      >
        {{ t('plugins.settings.cancel') }}
      </NvxButton>
      <NvxButton
        :disabled="!snapshot || loading || busy || invalid || conflicted"
        @click="save()"
      >
        {{ t(busy ? 'plugins.settings.busy' : 'plugins.settings.save') }}
      </NvxButton>
    </template>
  </NvxDialog>
</template>

<style scoped>
.plugin-settings { display: grid; min-width: 0; gap: var(--nvx-space-4); }
.plugin-settings__reset { margin-inline-end: auto; }
</style>
