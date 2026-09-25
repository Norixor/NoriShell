<script setup lang="ts">
import { isTauri } from "@tauri-apps/api/core";
import { computed, onBeforeUnmount, ref, useTemplateRef } from "vue";
import { useI18n } from "vue-i18n";

import { createPreferenceAdapters } from "../../preference-adapters";
import { exportJsonFile } from "../../platform-file-export";
import {
  applyPreferencePreview, exportPreferenceTransfer, MAX_PREFERENCE_TRANSFER_BYTES,
  parsePreferenceTransfer, PREFERENCE_GROUP_IDS, previewPreferenceTransfer,
  type PreferenceGroupId, type PreferencePreviewGroup, type PreferenceTransferFile,
} from "../../preferences-transfer";
import { NvxButton, NvxCheckbox, NvxInlineNotice, NvxStatusLabel } from "../ui";

const { t } = useI18n();
const adapters = createPreferenceAdapters();
const fileInput = useTemplateRef<HTMLInputElement>("fileInput");
const visibleGroups = PREFERENCE_GROUP_IDS.filter((id) => id !== "commandNotifications");
const selected = ref<PreferenceGroupId[]>(visibleGroups.filter((id) => isTauri() || id !== "desktop"));
const loadedFile = ref<PreferenceTransferFile | null>(null);
const preview = ref<PreferencePreviewGroup[]>([]);
const busy = ref(false);
const error = ref("");
const message = ref("");
const mode = ref<"import" | "reset">("import");
let alive = true;
const selectedAdapters = computed(() => adapters.filter((adapter) => selected.value.includes(adapter.id)));
const hasPending = computed(() => preview.value.some((group) => group.result === "pending" && selected.value.includes(group.id)));
function toggle(id: PreferenceGroupId, checked: boolean) {
  selected.value = checked ? [...new Set([...selected.value, id])] : selected.value.filter((key) => key !== id);
}
function fail(caught: unknown) {
  const code = caught instanceof Error ? caught.message : "failed";
  error.value = ["tooLarge", "emptySelection", "invalidFile", "invalidGroup", "unavailableGroup"].includes(code) ? code : "failed";
}
async function perform(operation: () => Promise<void>) {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  message.value = "";
  try { await operation(); } catch (caught) { if (alive) fail(caught); }
  finally { if (alive) busy.value = false; }
}
async function exportFile() {
  await perform(async () => {
    const text = await exportPreferenceTransfer(selectedAdapters.value);
    if (!alive) return;
    if (!await exportJsonFile("preferences", text) || !alive) return;
    message.value = "exported";
  });
}
async function readFile(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  await perform(async () => {
    if (file.size > MAX_PREFERENCE_TRANSFER_BYTES) throw new Error("tooLarge");
    const parsed = parsePreferenceTransfer(await file.text(), adapters);
    if (!alive) return;
    loadedFile.value = parsed;
    selected.value = visibleGroups.filter((id) => Object.hasOwn(parsed.groups, id));
    preview.value = [];
    mode.value = "import";
    message.value = "loaded";
  });
}
async function buildPreview(reset: boolean) {
  await perform(async () => {
    if (!selected.value.length) throw new Error("emptySelection");
    const groups: PreferenceTransferFile["groups"] = {};
    for (const adapter of selectedAdapters.value) {
      if (reset) groups[adapter.id] = adapter.defaults();
      else if (loadedFile.value && Object.hasOwn(loadedFile.value.groups, adapter.id)) groups[adapter.id] = loadedFile.value.groups[adapter.id];
    }
    if (!Object.keys(groups).length) throw new Error("emptySelection");
    const next = await previewPreferenceTransfer({ product: "NoriShell", version: 1, groups }, adapters);
    if (!alive) return;
    preview.value = next;
    mode.value = reset ? "reset" : "import";
  });
}
async function apply() {
  await perform(async () => { await applyPreferencePreview(preview.value, new Set(selected.value), adapters, () => alive); });
}
const pretty = (text: string) => JSON.stringify(JSON.parse(text), null, 2);
onBeforeUnmount(() => {
  alive = false;
});
</script>

<template>
  <section
    class="preference-transfer"
    aria-labelledby="preference-transfer-title"
  >
    <header>
      <h2 id="preference-transfer-title">
        {{ t('preferenceTransfer.title') }}
      </h2>
      <p>{{ t('preferenceTransfer.description') }}</p>
    </header>
    <NvxInlineNotice tone="info">
      {{ t('preferenceTransfer.exclusions') }}
    </NvxInlineNotice>
    <fieldset
      :disabled="busy"
      class="preference-transfer__groups"
    >
      <legend>{{ t('preferenceTransfer.groupsTitle') }}</legend>
      <NvxCheckbox
        v-for="id in visibleGroups"
        :key="id"
        :model-value="selected.includes(id)"
        :disabled="busy"
        @update:model-value="toggle(id, $event)"
      >
        {{ t(`preferenceTransfer.groups.${id}`) }}
      </NvxCheckbox>
    </fieldset>
    <div class="preference-transfer__actions">
      <NvxButton
        variant="secondary"
        :disabled="busy || !selected.length"
        @click="exportFile"
      >
        {{ t('preferenceTransfer.export') }}
      </NvxButton>
      <NvxButton
        variant="secondary"
        :disabled="busy"
        @click="fileInput?.click()"
      >
        {{ t('preferenceTransfer.chooseFile') }}
      </NvxButton>
      <NvxButton
        v-if="loadedFile"
        variant="secondary"
        :disabled="busy || !selected.length"
        @click="buildPreview(false)"
      >
        {{ t('preferenceTransfer.previewImport') }}
      </NvxButton>
      <NvxButton
        variant="secondary"
        :disabled="busy || !selected.length"
        @click="buildPreview(true)"
      >
        {{ t('preferenceTransfer.previewReset') }}
      </NvxButton>
      <input
        ref="fileInput"
        type="file"
        accept=".json,application/json"
        hidden
        @change="readFile"
      >
    </div>
    <NvxInlineNotice
      v-if="error"
      tone="error"
    >
      {{ t(`preferenceTransfer.errors.${error}`) }}
    </NvxInlineNotice>
    <p
      v-if="message"
      role="status"
    >
      {{ t(`preferenceTransfer.${message}`) }}
    </p>
    <template v-if="preview.length">
      <h3>{{ t(`preferenceTransfer.${mode === 'reset' ? 'resetPreview' : 'importPreview'}`) }}</h3>
      <p>{{ t('preferenceTransfer.previewHint') }}</p>
      <article
        v-for="group in preview"
        :key="group.id"
        class="preference-transfer__group"
      >
        <div class="preference-transfer__group-heading">
          <strong>{{ t(`preferenceTransfer.groups.${group.id}`) }}</strong>
          <NvxStatusLabel :tone="group.result === 'failed' || group.result === 'conflict' ? 'warning' : group.result === 'applied' ? 'success' : 'neutral'">
            {{ t(`preferenceTransfer.results.${group.result}`) }}
          </NvxStatusLabel>
        </div>
        <p v-if="group.before === group.after">
          {{ t('preferenceTransfer.noChanges') }}
        </p>
        <details v-else>
          <summary>{{ t('preferenceTransfer.compare') }}</summary>
          <div class="preference-transfer__comparison">
            <div><h4>{{ t('preferenceTransfer.before') }}</h4><pre>{{ pretty(group.before) }}</pre></div>
            <div><h4>{{ t('preferenceTransfer.after') }}</h4><pre>{{ pretty(group.after) }}</pre></div>
          </div>
        </details>
      </article>
      <NvxButton
        :disabled="busy || !hasPending"
        @click="apply"
      >
        {{ t(`preferenceTransfer.${mode === 'reset' ? 'applyReset' : 'applyImport'}`) }}
      </NvxButton>
    </template>
  </section>
</template>

<style scoped>
.preference-transfer { display: grid; gap: var(--nvx-space-4); max-width: 960px; }
.preference-transfer h2, .preference-transfer h3, .preference-transfer h4 { margin: 0; }
.preference-transfer p { margin: 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); line-height: 1.6; }
.preference-transfer__groups { border: 0; padding: 0; margin: 0; display: grid; gap: var(--nvx-space-3); grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); }
.preference-transfer__groups legend { margin-bottom: var(--nvx-space-3); }
.preference-transfer__actions { display: flex; flex-wrap: wrap; gap: var(--nvx-space-2); }
.preference-transfer__group { display: grid; gap: var(--nvx-space-2); border-top: var(--nvx-border-width) solid var(--nvx-color-border); padding-top: var(--nvx-space-3); }
.preference-transfer__group-heading { display: flex; align-items: center; justify-content: space-between; gap: var(--nvx-space-2); }
.preference-transfer__comparison { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: var(--nvx-space-3); margin-top: var(--nvx-space-3); }
.preference-transfer__comparison > div { min-width: 0; }
.preference-transfer pre { max-height: 300px; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; font-size: var(--nvx-font-size-xs); background: var(--nvx-color-bg-subtle); padding: var(--nvx-space-3); }
.preference-transfer summary { cursor: pointer; color: var(--nvx-color-accent); }
</style>
