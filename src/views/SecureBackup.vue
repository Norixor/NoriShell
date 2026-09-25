<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";
import { NvxButton, NvxCheckbox, NvxField, NvxInlineNotice, NvxInput } from "../components/ui";
import { secureBackupClient, type SecureBackupPrompt } from "../core-api/offline-backup-client";
import { MIN_NEW_SECRET_PASSWORD_CHARACTERS, passwordCharacterCount } from "../password-policy";

const { t } = useI18n();
const id = new URLSearchParams(window.location.search).get("prompt") ?? "";
const prompt = ref<SecureBackupPrompt | null>(null);
const password = ref("");
const confirmation = ref("");
const confirmed = ref(false);
const pending = ref(false);
const failed = ref(false);
const exporting = computed(() => prompt.value?.kind === "export");
const restoring = computed(() => prompt.value?.kind === "restoreVault" || prompt.value?.kind === "mergeVault");
const requiresConfirmation = computed(() => exporting.value || restoring.value);
const passwordTooShort = computed(() => exporting.value && !!password.value
  && passwordCharacterCount(password.value) < MIN_NEW_SECRET_PASSWORD_CHARACTERS);
const openPasswordTooShort = computed(() => prompt.value?.kind === "open" && !!password.value
  && new TextEncoder().encode(password.value).byteLength < 8);
const confirmationMismatch = computed(() => exporting.value && !!confirmation.value
  && password.value !== confirmation.value);
const valid = computed(() => !!prompt.value
  && (exporting.value
    ? passwordCharacterCount(password.value) >= MIN_NEW_SECRET_PASSWORD_CHARACTERS
    : prompt.value?.kind === "open"
      ? new TextEncoder().encode(password.value).byteLength >= 8
      : !!password.value)
  && (!exporting.value || password.value === confirmation.value)
  && (!requiresConfirmation.value || confirmed.value));
const title = computed(() => t(`offlineBackup.secure.${prompt.value?.kind ?? "open"}.title`));
const description = computed(() => t(`offlineBackup.secure.${prompt.value?.kind ?? "open"}.description`));

function clear() {
  password.value = "";
  confirmation.value = "";
  confirmed.value = false;
}

async function submit() {
  if (pending.value || !valid.value) return;
  pending.value = true;
  failed.value = false;
  const request = secureBackupClient.submit(id, password.value, confirmation.value, confirmed.value);
  clear();
  try {
    await request;
  } catch {
    failed.value = true;
    pending.value = false;
  }
}

async function cancel() {
  if (pending.value) return;
  clear();
  pending.value = true;
  try {
    await secureBackupClient.cancel(id);
  } catch {
    failed.value = true;
    pending.value = false;
  }
}

onMounted(async () => {
  try {
    prompt.value = await secureBackupClient.get(id);
  } catch {
    failed.value = true;
  }
});
onBeforeUnmount(clear);
</script>

<template>
  <NvxSecureWindow
    :title="title"
    :description="description"
    layout="form"
  >
    <form
      class="secure-backup-form"
      @submit.prevent="submit"
    >
      <NvxInlineNotice
        v-if="requiresConfirmation"
        tone="warning"
      >
        {{ t(restoring ? (prompt?.kind === "mergeVault" ? "offlineBackup.vaultMergePreview" : "offlineBackup.vaultPreview") : "offlineBackup.exportRisk") }}
      </NvxInlineNotice>
      <NvxField
        for-id="secure-backup-password"
        :label="t(restoring ? 'offlineBackup.originalVaultPassword' : 'offlineBackup.password')"
        :hint="t(restoring ? (prompt?.kind === 'mergeVault' ? 'offlineBackup.originalVaultMergePasswordHint' : 'offlineBackup.originalVaultPasswordHint') : exporting ? 'offlineBackup.passwordLength' : 'offlineBackup.openPasswordLength')"
        :error="passwordTooShort ? t('offlineBackup.passwordLength') : openPasswordTooShort ? t('offlineBackup.openPasswordLength') : undefined"
      >
        <NvxInput
          id="secure-backup-password"
          v-model="password"
          type="password"
          :autocomplete="exporting ? 'new-password' : 'off'"
          :disabled="pending || !prompt"
          :invalid="passwordTooShort || openPasswordTooShort"
          autofocus
        />
      </NvxField>
      <NvxField
        v-if="exporting"
        for-id="secure-backup-confirmation"
        :label="t('offlineBackup.confirmPassword')"
        :hint="t('offlineBackup.confirmPasswordHint')"
        :error="confirmationMismatch ? t('offlineBackup.passwordMismatch') : undefined"
      >
        <NvxInput
          id="secure-backup-confirmation"
          v-model="confirmation"
          type="password"
          autocomplete="new-password"
          :disabled="pending"
          :invalid="confirmationMismatch"
        />
      </NvxField>
      <NvxCheckbox
        v-if="requiresConfirmation"
        v-model="confirmed"
        :disabled="pending"
      >
        {{ t(restoring ? (prompt?.kind === "mergeVault" ? "offlineBackup.secure.mergeVault.confirm" : "offlineBackup.secure.restoreVault.confirm") : "offlineBackup.secure.export.confirm") }}
      </NvxCheckbox>
      <NvxInlineNotice
        v-if="failed"
        tone="error"
      >
        {{ t("offlineBackup.secure.failed") }}
      </NvxInlineNotice>
      <button
        type="submit"
        hidden
        tabindex="-1"
      />
    </form>
    <template #actions>
      <NvxButton
        variant="secondary"
        :disabled="pending"
        @click="cancel"
      >
        {{ t("offlineBackup.secure.cancel") }}
      </NvxButton>
      <NvxButton
        :loading="pending"
        :disabled="!valid || pending"
        @click="submit"
      >
        {{ t("offlineBackup.secure.continue") }}
      </NvxButton>
    </template>
  </NvxSecureWindow>
</template>

<style scoped>
.secure-backup-form { display: grid; gap: var(--nvx-space-4); }
</style>
