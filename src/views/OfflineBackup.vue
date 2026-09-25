<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import { useI18n } from "vue-i18n";

import {
  applyOfflineBackup,
  discardOfflineBackup,
  exportOfflineBackup,
  openOfflineBackup,
  previewOfflineBackup,
  type OfflineBackupPreview,
  type OfflineBackupSelection,
  type OfflineDuplicatePolicy,
  type OfflineVaultImportMode,
  type OpenedOfflineBackup,
} from "../core-api/offline-backup-client";
import { fetchVaultStatus } from "../core-api/client";
import { requestSecureVault } from "../core-api/secure-vault-client";
import { useTipsStore } from "../stores/tips";
import {
  NvxButton,
  NvxCheckbox,
  NvxField,
  NvxInlineNotice,
  NvxSelect,
} from "../components/ui";

const { t } = useI18n();
const tips = useTipsStore();
const exportSelection = ref<OfflineBackupSelection>({
  ssh: true,
  desktop: true,
  credentials: false,
  vault: false,
});
const importSelection = ref<OfflineBackupSelection>({
  ssh: false,
  desktop: false,
  credentials: false,
  vault: false,
});
const opened = ref<OpenedOfflineBackup | null>(null);
const preview = ref<OfflineBackupPreview | null>(null);
const previewVaultMode = ref<OfflineVaultImportMode | null>(null);
const duplicatePolicy = ref<OfflineDuplicatePolicy>("skip");
const busy = ref(false);
const recoveryError = ref("");

const canExport = computed(() =>
  exportSelection.value.ssh || exportSelection.value.desktop || exportSelection.value.vault,
);
const canImport = computed(() => {
  const selection = importSelection.value;
  return selection.ssh || selection.desktop || selection.vault;
});
const duplicateOptions = computed(() => [
  { value: "skip", label: t("offlineBackup.duplicateSkip") },
  { value: "copy", label: t("offlineBackup.duplicateCopy") },
]);

onBeforeUnmount(() => {
  if (opened.value) void discardOfflineBackup(opened.value.handle).catch(() => undefined);
});

function updateExport(key: keyof OfflineBackupSelection, value: boolean) {
  exportSelection.value = { ...exportSelection.value, [key]: value };
  if (!exportSelection.value.ssh && !exportSelection.value.desktop) {
    exportSelection.value.credentials = false;
  }
}

function updateImport(key: keyof OfflineBackupSelection, value: boolean) {
  importSelection.value = { ...importSelection.value, [key]: value };
  if (!importSelection.value.ssh && !importSelection.value.desktop) {
    importSelection.value.credentials = false;
  }
  preview.value = null;
  previewVaultMode.value = null;
}

function clearMessages() {
  recoveryError.value = "";
  tips.dismissScope("offline-backup");
}

function showError(key: string) {
  tips.show({ scope: "offline-backup", tone: "error", title: t(key) });
}

async function exportFile() {
  clearMessages();
  if (!canExport.value) {
    showError("offlineBackup.invalidSelection");
    return;
  }
  busy.value = true;
  try {
    if (exportSelection.value.credentials || exportSelection.value.vault) {
      const unlocked = await requestSecureVault("ensureUnlocked");
      if (!unlocked) return;
    }
    const saved = await exportOfflineBackup({ ...exportSelection.value });
    if (saved) tips.show({ scope: "offline-backup", tone: "success", title: t("offlineBackup.exportSuccess") });
  } catch {
    showError("offlineBackup.exportFailed");
  } finally {
    busy.value = false;
  }
}

async function openFile() {
  clearMessages();
  preview.value = null;
  previewVaultMode.value = null;
  const previous = opened.value;
  opened.value = null;
  if (previous) await discardOfflineBackup(previous.handle).catch(() => undefined);
  busy.value = true;
  try {
    const result = await openOfflineBackup();
    if (!result) return;
    opened.value = result;
    importSelection.value = {
      ssh: result.inventory.ssh,
      desktop: result.inventory.desktop,
      credentials: false,
      vault: false,
    };
  } catch {
    showError("offlineBackup.openFailed");
  } finally {
    busy.value = false;
  }
}

async function previewImport() {
  clearMessages();
  preview.value = null;
  previewVaultMode.value = null;
  if (!opened.value || !canImport.value) {
    showError("offlineBackup.invalidSelection");
    return;
  }
  busy.value = true;
  try {
    let vaultMode: OfflineVaultImportMode | null = null;
    if (importSelection.value.vault) {
      let status = await fetchVaultStatus();
      if (status.state === "locked") {
        const unlocked = await requestSecureVault("ensureUnlocked");
        if (!unlocked) return;
        status = await fetchVaultStatus();
      }
      if (status.state === "missing") vaultMode = "fresh";
      else if (status.state === "unlocked") vaultMode = "merge";
      else {
        showError("offlineBackup.vaultUnavailable");
        return;
      }
    }
    preview.value = await previewOfflineBackup(
      opened.value.handle,
      { ...importSelection.value },
      duplicatePolicy.value,
      vaultMode,
    );
    previewVaultMode.value = vaultMode;
  } catch (failure) {
    preview.value = null;
    previewVaultMode.value = null;
    showError(String(failure).includes("offline-backup-vault-already-exists")
      ? "offlineBackup.targetNotFresh"
      : "offlineBackup.previewFailed");
  } finally {
    busy.value = false;
  }
}

async function applyImport() {
  clearMessages();
  if (!opened.value || !preview.value) return;
  busy.value = true;
  try {
    if (previewVaultMode.value === "merge" || (importSelection.value.credentials && !importSelection.value.vault)) {
      const unlocked = await requestSecureVault("ensureUnlocked");
      if (!unlocked) return;
    }
    await applyOfflineBackup(opened.value.handle, preview.value.handle);
    tips.show({ scope: "offline-backup", tone: "success", title: t("offlineBackup.importSuccess") });
    opened.value = null;
    preview.value = null;
    previewVaultMode.value = null;
    window.dispatchEvent(new Event("norishell:vault-changed"));
  } catch (failure) {
    if (String(failure).includes("offline-backup-cancelled")) return;
    const key = String(failure).includes("offline-backup-vault-restored-configs-failed")
      ? "offlineBackup.vaultRestoredPartial"
      : String(failure).includes("offline-backup-vault-merge-uncertain")
        ? "offlineBackup.vaultMergeUncertain"
        : String(failure).includes("offline-backup-vault-merge-failed")
          ? "offlineBackup.vaultMergeFailed"
          : String(failure).includes("offline-backup-target-not-fresh")
            ? "offlineBackup.targetNotFresh"
            : "offlineBackup.importFailed";
    if (key === "offlineBackup.vaultRestoredPartial" || key === "offlineBackup.vaultMergeUncertain") {
      recoveryError.value = t(key);
    } else {
      showError(key);
    }
    opened.value = null;
    preview.value = null;
    previewVaultMode.value = null;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <section
    class="offline-backup"
    aria-labelledby="offline-backup-title"
  >
    <header>
      <h3 id="offline-backup-title">
        {{ t("offlineBackup.title") }}
      </h3>
      <p>{{ t("offlineBackup.description") }}</p>
    </header>

    <div class="offline-backup__panels">
      <section
        class="offline-backup__panel"
        aria-labelledby="offline-backup-export-title"
      >
        <h4 id="offline-backup-export-title">
          {{ t("offlineBackup.exportTitle") }}
        </h4>
        <fieldset :disabled="busy">
          <legend class="sr-only">
            {{ t("offlineBackup.exportTitle") }}
          </legend>
          <NvxCheckbox
            :model-value="exportSelection.ssh"
            @update:model-value="updateExport('ssh', $event)"
          >
            {{ t("offlineBackup.ssh") }}
          </NvxCheckbox>
          <NvxCheckbox
            :model-value="exportSelection.desktop"
            @update:model-value="updateExport('desktop', $event)"
          >
            {{ t("offlineBackup.desktop") }}
          </NvxCheckbox>
          <p class="offline-backup__hint">
            {{ t("offlineBackup.dependencyHint") }}
          </p>
          <NvxCheckbox
            :model-value="exportSelection.credentials"
            :disabled="!exportSelection.ssh && !exportSelection.desktop"
            @update:model-value="updateExport('credentials', $event)"
          >
            {{ t("offlineBackup.credentials") }}
          </NvxCheckbox>
          <p class="offline-backup__hint">
            {{ t("offlineBackup.credentialsHint") }}
          </p>
          <NvxCheckbox
            :model-value="exportSelection.vault"
            @update:model-value="updateExport('vault', $event)"
          >
            {{ t("offlineBackup.vault") }}
          </NvxCheckbox>
          <p class="offline-backup__hint">
            {{ t("offlineBackup.vaultHint") }}
          </p>
        </fieldset>
        <NvxInlineNotice tone="warning">
          {{ t("offlineBackup.exportRisk") }}
        </NvxInlineNotice>
        <NvxButton
          size="sm"
          :loading="busy"
          :loading-label="t('offlineBackup.busy')"
          :disabled="!canExport"
          @click="exportFile"
        >
          {{ t("offlineBackup.exportAction") }}
        </NvxButton>
      </section>

      <section
        class="offline-backup__panel"
        aria-labelledby="offline-backup-import-title"
      >
        <h4 id="offline-backup-import-title">
          {{ t("offlineBackup.importTitle") }}
        </h4>
        <NvxInlineNotice tone="info">
          {{ t("offlineBackup.importRisk") }}
        </NvxInlineNotice>
        <NvxButton
          size="sm"
          variant="secondary"
          :loading="busy"
          :loading-label="t('offlineBackup.busy')"
          @click="openFile"
        >
          {{ t("offlineBackup.openAction") }}
        </NvxButton>

        <template v-if="opened">
          <fieldset :disabled="busy">
            <legend>{{ t("offlineBackup.selectedContents") }}</legend>
            <p class="offline-backup__hint">
              {{ t("offlineBackup.inventory", { hosts: opened.inventory.hostCount, desktops: opened.inventory.desktopCount, credentials: opened.inventory.credentialCount }) }}
            </p>
            <NvxCheckbox
              v-if="opened.inventory.ssh"
              :model-value="importSelection.ssh"
              @update:model-value="updateImport('ssh', $event)"
            >
              {{ t("offlineBackup.ssh") }}
            </NvxCheckbox>
            <NvxCheckbox
              v-if="opened.inventory.desktop"
              :model-value="importSelection.desktop"
              @update:model-value="updateImport('desktop', $event)"
            >
              {{ t("offlineBackup.desktop") }}
            </NvxCheckbox>
            <NvxCheckbox
              v-if="opened.inventory.credentials"
              :model-value="importSelection.credentials"
              :disabled="!importSelection.ssh && !importSelection.desktop"
              @update:model-value="updateImport('credentials', $event)"
            >
              {{ t("offlineBackup.credentials") }}
            </NvxCheckbox>
            <NvxCheckbox
              v-if="opened.inventory.vault"
              :model-value="importSelection.vault"
              @update:model-value="updateImport('vault', $event)"
            >
              {{ t("offlineBackup.vault") }}
            </NvxCheckbox>
          </fieldset>
          <NvxField
            v-if="importSelection.ssh || importSelection.desktop"
            :label="t('offlineBackup.duplicatePolicy')"
          >
            <NvxSelect
              :model-value="duplicatePolicy"
              :options="duplicateOptions"
              :disabled="busy"
              @update:model-value="duplicatePolicy = $event as OfflineDuplicatePolicy; preview = null; previewVaultMode = null"
            />
          </NvxField>
          <NvxButton
            size="sm"
            variant="secondary"
            :disabled="busy || !canImport"
            @click="previewImport"
          >
            {{ t("offlineBackup.previewAction") }}
          </NvxButton>
        </template>

        <template v-if="preview">
          <p>{{ t("offlineBackup.preview", { hosts: preview.hostCount, desktops: preview.desktopProfileCount, credentials: preview.credentialCount, duplicateHosts: preview.duplicateHostCount, duplicateDesktops: preview.duplicateDesktopProfileCount, skipped: preview.skippedCount }) }}</p>
          <NvxInlineNotice
            v-if="importSelection.vault"
            tone="warning"
          >
            {{ t(previewVaultMode === "merge" ? "offlineBackup.vaultMergePreview" : "offlineBackup.vaultPreview") }}
          </NvxInlineNotice>
          <NvxButton
            size="sm"
            :loading="busy"
            :loading-label="t('offlineBackup.busy')"
            @click="applyImport"
          >
            {{ t("offlineBackup.applyAction") }}
          </NvxButton>
        </template>
      </section>
    </div>

    <NvxInlineNotice
      v-if="recoveryError"
      tone="error"
    >
      {{ recoveryError }}
    </NvxInlineNotice>
  </section>
</template>

<style scoped>
.offline-backup { display: grid; gap: var(--nvx-space-4); }
.offline-backup header h3, .offline-backup__panel h4 { margin: 0; }
.offline-backup header p, .offline-backup__hint { color: var(--nvx-color-text-secondary); }
.offline-backup__panels { display: grid; gap: var(--nvx-space-4); grid-template-columns: repeat(auto-fit, minmax(min(100%, 300px), 1fr)); }
.offline-backup__panel { display: grid; align-content: start; gap: var(--nvx-space-3); padding: var(--nvx-space-4); border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); }
.offline-backup fieldset { display: grid; gap: var(--nvx-space-2); min-width: 0; margin: 0; padding: 0; border: 0; }
.offline-backup fieldset legend { margin-bottom: var(--nvx-space-2); font-weight: var(--nvx-font-weight-medium); }
.offline-backup__hint { margin: 0; font-size: var(--nvx-font-size-sm); }
.offline-backup__panel > .nvx-button { justify-self: start; }
.sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border: 0; }
</style>
