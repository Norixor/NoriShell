<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";
import { NvxButton, NvxCheckbox, NvxField, NvxInlineNotice, NvxInput } from "../components/ui";
import { secureVaultClient, type SecureVaultPrompt } from "../core-api/secure-vault-client";
import { parseCoreApiError } from "../core-api/client";
import { MIN_NEW_SECRET_PASSWORD_CHARACTERS, passwordCharacterCount } from "../password-policy";
const { t, te } = useI18n();
const id = new URLSearchParams(window.location.search).get("prompt") ?? "";
const prompt = ref<SecureVaultPrompt | null>(null);
const password = ref("");
const confirmation = ref("");
const confirmed = ref(false);
const pending = ref(false);
const failed = ref(false);
const failureText = ref("");
function showFailure(error: unknown) {
  const failure = parseCoreApiError(error);
  failureText.value = (failure?.messageKey && te(failure.messageKey) ? t(failure.messageKey) : t("sshHosts.vault.failed"))
    + (failure?.diagnosticId ? ` ${t("diagnostics.id", { id: failure.diagnosticId })}` : "");
  failed.value = true;
}
const resetStep = ref(false);
const resetPhrase = ref("");
const resetConfirmed = ref(false);
const resetError = ref<"failed" | "changed" | "uncertain" | null>(null);
const resetFailureText = ref("");
const creating = computed(() => prompt.value?.kind === "create");
const canReset = computed(() => prompt.value?.kind === "unlock" && prompt.value.canReset);
const automatic = computed(() => prompt.value?.kind === "enableAutoUnlock");
const localAutomatic = computed(() => prompt.value?.kind === "enableLocalAutoUnlock");
const requiresAutoUnlockConfirmation = computed(() => automatic.value || localAutomatic.value);
const autoUnlockDialogKey = computed(() => (
  localAutomatic.value ? "sshSettings.vault.enableLocalDialog" : "sshSettings.vault.enableDialog"
));
const passwordTooShort = computed(() => creating.value && !!password.value
  && passwordCharacterCount(password.value) < MIN_NEW_SECRET_PASSWORD_CHARACTERS);
const confirmationMismatch = computed(() => creating.value && !!confirmation.value
  && password.value !== confirmation.value);
const valid = computed(() => (
  !!prompt.value
  && !!password.value
  && (!creating.value || (
    passwordCharacterCount(password.value) >= MIN_NEW_SECRET_PASSWORD_CHARACTERS
    && password.value === confirmation.value
  ))
  && (!requiresAutoUnlockConfirmation.value || confirmed.value)
));
const resetReady = computed(() => canReset.value && resetStep.value
  && resetPhrase.value === "RESET" && resetConfirmed.value);
const title = computed(() => t(
  resetStep.value ? "sshHosts.vault.reset.title" : requiresAutoUnlockConfirmation.value
    ? `${autoUnlockDialogKey.value}.title`
    : creating.value ? "sshHosts.vault.createTitle" : "sshHosts.vault.unlockTitle",
));
const description = computed(() => t(
  resetStep.value ? "sshHosts.vault.reset.description" : requiresAutoUnlockConfirmation.value
    ? `${autoUnlockDialogKey.value}.description`
    : creating.value ? "plugins.sshSync.prompt.createLocalVault.description" : "sshHosts.vault.description",
));
function clear() { password.value = ""; confirmation.value = ""; confirmed.value = false; }
function clearReset() { resetPhrase.value = ""; resetConfirmed.value = false; }
function enterReset() {
  if (pending.value || !canReset.value) return;
  clear();
  failed.value = false;
  resetError.value = null;
  resetFailureText.value = "";
  resetStep.value = true;
}
function leaveReset() {
  if (pending.value) return;
  clearReset();
  resetError.value = null;
  resetFailureText.value = "";
  resetStep.value = false;
}
async function submit() {
  if (pending.value || !valid.value) return;
  pending.value = true;
  failed.value = false;
  const request = secureVaultClient.submit(id, password.value, confirmation.value, confirmed.value);
  clear();
  try { await request; } catch (error) { showFailure(error); pending.value = false; }
}
async function cancel() {
  if (pending.value) return;
  clear();
  pending.value = true;
  try { await secureVaultClient.cancel(id); } catch (error) { showFailure(error); pending.value = false; }
}
async function submitReset() {
  if (pending.value || !resetReady.value) return;
  pending.value = true;
  resetError.value = null;
  resetFailureText.value = "";
  const request = secureVaultClient.reset(id, resetPhrase.value, resetConfirmed.value);
  clearReset();
  try {
    await request;
  } catch (error) {
    const code = parseCoreApiError(error)?.code;
    resetError.value = code === "secureVaultResetChanged" ? "changed"
      : code === "secureVaultResetUncertain" ? "uncertain" : "failed";
    const failure = parseCoreApiError(error);
    if (resetError.value === "failed" && failure?.messageKey && te(failure.messageKey)) {
      resetFailureText.value = t(failure.messageKey);
    }
    if (resetError.value === "failed" && failure?.diagnosticId) {
      resetFailureText.value += ` ${t("diagnostics.id", { id: failure.diagnosticId })}`;
    }
    pending.value = false;
  }
}
onMounted(async () => { try { prompt.value = await secureVaultClient.get(id); } catch (error) { showFailure(error); } });
onBeforeUnmount(() => { clear(); clearReset(); });
</script>

<template>
  <NvxSecureWindow
    :title="title"
    :description="description"
    layout="form"
    :danger="resetStep"
  >
    <form
      class="secure-vault-form"
      @submit.prevent="resetStep ? submitReset() : submit()"
    >
      <template v-if="!resetStep">
        <NvxInlineNotice
          v-if="requiresAutoUnlockConfirmation"
          tone="warning"
        >
          {{ t(`${autoUnlockDialogKey}.securityNotice`) }}
        </NvxInlineNotice>
        <NvxField
          for-id="secure-vault-password"
          :label="t(localAutomatic ? 'sshSettings.vault.enableLocalDialog.password' : 'sshHosts.vault.password')"
          :hint="t(creating ? 'sshTerminal.vaultPasswordHint' : 'sshTerminal.vaultPasswordRequired')"
          :error="passwordTooShort ? t('sshTerminal.vaultPasswordTooShort') : undefined"
        >
          <NvxInput
            id="secure-vault-password"
            v-model="password"
            type="password"
            :autocomplete="creating ? 'new-password' : 'current-password'"
            :disabled="pending || !prompt"
            :invalid="passwordTooShort"
            autofocus
          />
        </NvxField>
        <NvxField
          v-if="creating"
          for-id="secure-vault-confirmation"
          :label="t('sshHosts.vault.passwordConfirmation')"
          :hint="t('sshTerminal.confirmVaultPassword')"
          :error="confirmationMismatch ? t('sshTerminal.vaultPasswordMismatch') : undefined"
        >
          <NvxInput
            id="secure-vault-confirmation"
            v-model="confirmation"
            type="password"
            autocomplete="new-password"
            :disabled="pending"
            :invalid="confirmationMismatch"
          />
        </NvxField>
        <NvxCheckbox
          v-if="requiresAutoUnlockConfirmation"
          v-model="confirmed"
          :disabled="pending"
        >
          {{ t(`${autoUnlockDialogKey}.confirm`) }}
        </NvxCheckbox>
        <NvxButton
          v-if="canReset"
          class="secure-vault-reset-link"
          variant="ghost"
          size="sm"
          :disabled="pending"
          @click="enterReset"
        >
          {{ t("sshHosts.vault.reset.open") }}
        </NvxButton>
        <NvxInlineNotice
          v-if="failed"
          tone="error"
        >
          {{ failureText || t("sshHosts.vault.failed") }}
        </NvxInlineNotice>
      </template>
      <template v-else>
        <NvxInlineNotice tone="warning">
          {{ t("sshHosts.vault.reset.warning") }}
        </NvxInlineNotice>
        <NvxField
          for-id="secure-vault-reset-phrase"
          :label="t('sshHosts.vault.reset.phraseLabel')"
          :hint="t('sshHosts.vault.reset.phraseHint')"
        >
          <NvxInput
            id="secure-vault-reset-phrase"
            v-model="resetPhrase"
            type="text"
            autocomplete="off"
            spellcheck="false"
            :disabled="pending"
            autofocus
          />
        </NvxField>
        <NvxCheckbox
          v-model="resetConfirmed"
          :disabled="pending"
        >
          {{ t("sshHosts.vault.reset.confirm") }}
        </NvxCheckbox>
        <NvxInlineNotice
          v-if="resetError"
          tone="error"
        >
          {{ resetFailureText || t(`sshHosts.vault.reset.${resetError}`) }}
        </NvxInlineNotice>
      </template>
      <button
        type="submit"
        hidden
        tabindex="-1"
      />
    </form>
    <template #actions>
      <template v-if="resetStep">
        <NvxButton
          variant="secondary"
          :disabled="pending"
          @click="leaveReset"
        >
          {{ t("sshHosts.vault.reset.back") }}
        </NvxButton>
        <NvxButton
          variant="danger"
          :loading="pending"
          :disabled="!resetReady || pending"
          @click="submitReset"
        >
          {{ t("sshHosts.vault.reset.delete") }}
        </NvxButton>
      </template>
      <template v-else>
        <NvxButton
          variant="secondary"
          :disabled="pending"
          @click="cancel"
        >
          {{ t(requiresAutoUnlockConfirmation ? `${autoUnlockDialogKey}.cancel` : "sshSettings.vault.enableDialog.cancel") }}
        </NvxButton>
        <NvxButton
          :loading="pending"
          :disabled="!valid || pending"
          @click="submit"
        >
          {{ t(requiresAutoUnlockConfirmation ? `${autoUnlockDialogKey}.enable` : creating ? "sshHosts.vault.createAction" : "sshHosts.vault.unlockAction") }}
        </NvxButton>
      </template>
    </template>
  </NvxSecureWindow>
</template>

<style scoped>
.secure-vault-form { display: grid; gap: var(--nvx-space-4); }
.secure-vault-reset-link { justify-self: start; color: var(--nvx-color-danger); }
</style>
