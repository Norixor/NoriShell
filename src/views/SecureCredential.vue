<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";
import { NvxButton, NvxField, NvxInlineNotice, NvxInput, NvxTextarea } from "../components/ui";
import { secureCredentialClient, type SecureCredentialPrompt } from "../core-api/secure-credential-client";
import { requestSecureVault } from "../core-api/secure-vault-client";
import { parseCoreApiError } from "../core-api/client";

const { t, te } = useI18n();
const id = new URLSearchParams(window.location.search).get("prompt") ?? "";
const prompt = ref<SecureCredentialPrompt | null>(null);
const secret = ref("");
const passphrase = ref("");
const pending = ref(false);
const failed = ref(false);
const failureText = ref("");
// Passphrase problems are reported on the passphrase field, which is focused
// for the retry. The private-key text is the only value kept across that retry:
// it stays in this surface's memory while Core answers and after a passphrase
// error, and is cleared on success, any other error, cancel, expiry or close.
// Passwords and passphrases are cleared as soon as they are submitted.
const PASSPHRASE_CODES = new Set([
  "credential.passphrase_required",
  "credential.incorrect_passphrase",
  "credential_transient.passphrase_required",
  "credential_transient.incorrect_passphrase",
]);
const passphraseError = ref(false);
// Core reports unusable material with one code for both credential kinds; the prompt kind
// (not the message text) picks the wording, so a broken key file is named as such.
function failureMessageKey(failure: ReturnType<typeof parseCoreApiError>): string | null {
  if (!failure?.messageKey || !te(failure.messageKey)) return null;
  const invalidMaterial = failure.messageKey === "secureWindow.credential.invalidMaterial" || failure.code.endsWith(".invalid_material");
  if (invalidMaterial && prompt.value?.kind === "privateKey") return "secureWindow.credential.invalidPrivateKey";
  return failure.messageKey;
}
function showFailure(error: unknown) {
  const failure = parseCoreApiError(error);
  passphraseError.value = !!failure && PASSPHRASE_CODES.has(failure.code);
  const messageKey = failureMessageKey(failure);
  failureText.value = (messageKey ? t(messageKey) : t("sshTerminal.connectFailedBody"))
    + (failure?.diagnosticId ? ` ${t("diagnostics.id", { id: failure.diagnosticId })}` : "");
  failed.value = true;
}

const privateKey = computed(() => prompt.value?.kind === "privateKey");
const valid = computed(() => !!prompt.value && secret.value.length > 0);
const title = computed(() => prompt.value?.replacement
  ? t("identitySettings.changePasswordTitle")
  : t("sshSession.enterCredential"));
const description = computed(() => prompt.value?.replacement
  ? t("identitySettings.changePasswordDescription")
  : t("sshTerminal.reauthenticateDescription"));

function clear() {
  secret.value = "";
  passphrase.value = "";
}

// Core closes the prompt after 180 s; clear independently so the key text never
// outlives the prompt even if the window lingers.
const PROMPT_LIFETIME_MS = 180_000;
let expiry: ReturnType<typeof setTimeout> | undefined;

async function submit() {
  if (pending.value || !valid.value || !prompt.value) return;
  pending.value = true;
  failed.value = false;
  failureText.value = "";
  passphraseError.value = false;
  try {
    if (prompt.value.persistent && !await requestSecureVault("ensureUnlocked")) {
      pending.value = false;
      return;
    }
    const request = secureCredentialClient.submit(id, secret.value, privateKey.value ? passphrase.value : "");
    passphrase.value = "";
    if (!privateKey.value) secret.value = "";
    await request;
    clear();
  } catch (error) {
    showFailure(error);
    if (!(privateKey.value && passphraseError.value)) clear();
    pending.value = false;
    if (passphraseError.value) {
      await nextTick();
      document.getElementById("secure-credential-passphrase")?.focus();
    }
  }
}

async function cancel() {
  if (pending.value) return;
  pending.value = true;
  clear();
  try {
    await secureCredentialClient.cancel(id);
  } catch (error) {
    showFailure(error);
    pending.value = false;
  }
}

onMounted(async () => {
  expiry = setTimeout(clear, PROMPT_LIFETIME_MS);
  window.addEventListener("pagehide", clear);
  try {
    prompt.value = await secureCredentialClient.get(id);
  } catch (error) {
    showFailure(error);
  }
});
onBeforeUnmount(() => {
  clearTimeout(expiry);
  window.removeEventListener("pagehide", clear);
  clear();
});
</script>

<template>
  <NvxSecureWindow
    :title="title"
    :description="description"
    layout="form"
  >
    <form
      v-if="prompt"
      class="secure-credential-form"
      @submit.prevent="submit"
    >
      <p class="secure-credential-form__label">
        {{ prompt.label }}
      </p>
      <NvxField
        v-if="privateKey"
        for-id="secure-credential-private-key"
        :label="t('sshTerminal.privateKey')"
      >
        <NvxTextarea
          id="secure-credential-private-key"
          v-model="secret"
          :placeholder="t('sshTerminal.privateKeyPlaceholder')"
          :disabled="pending"
          :maxlength="1048576"
          autofocus
        />
      </NvxField>
      <NvxField
        v-else
        for-id="secure-credential-password"
        :label="t('sshTerminal.password')"
      >
        <NvxInput
          id="secure-credential-password"
          v-model="secret"
          type="password"
          autocomplete="off"
          :disabled="pending"
          :maxlength="65536"
          autofocus
        />
      </NvxField>
      <NvxField
        v-if="privateKey"
        for-id="secure-credential-passphrase"
        :label="t('sshTerminal.passphrase')"
        :error="passphraseError ? failureText : undefined"
      >
        <NvxInput
          id="secure-credential-passphrase"
          v-model="passphrase"
          type="password"
          autocomplete="off"
          :placeholder="t('sshTerminal.passphrasePlaceholder')"
          :disabled="pending"
          :invalid="passphraseError"
          :maxlength="65536"
        />
      </NvxField>
      <NvxInlineNotice
        v-if="failed && !passphraseError"
        tone="error"
      >
        {{ failureText || t("sshTerminal.connectFailedBody") }}
      </NvxInlineNotice>
      <button
        type="submit"
        hidden
        tabindex="-1"
      />
    </form>
    <NvxInlineNotice
      v-else-if="failed"
      tone="error"
    >
      {{ failureText || t("sshTerminal.connectFailedBody") }}
    </NvxInlineNotice>
    <template #actions>
      <NvxButton
        variant="secondary"
        :disabled="pending"
        @click="cancel"
      >
        {{ t("sshTerminal.cancel") }}
      </NvxButton>
      <NvxButton
        :loading="pending"
        :disabled="!valid || pending"
        @click="submit"
      >
        {{ prompt?.replacement ? t("identitySettings.changePasswordSave") : t("sshTerminal.connect") }}
      </NvxButton>
    </template>
  </NvxSecureWindow>
</template>

<style scoped>
.secure-credential-form { display: grid; gap: var(--nvx-space-4); }
.secure-credential-form__label { margin: 0; color: var(--nvx-color-text-secondary); }
</style>
