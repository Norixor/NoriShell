<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
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
function showFailure(error: unknown) {
  const failure = parseCoreApiError(error);
  failureText.value = (failure?.messageKey && te(failure.messageKey) ? t(failure.messageKey) : t("sshTerminal.connectFailedBody"))
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

async function submit() {
  if (pending.value || !valid.value || !prompt.value) return;
  pending.value = true;
  failed.value = false;
  failureText.value = "";
  try {
    if (prompt.value.persistent && !await requestSecureVault("ensureUnlocked")) {
      pending.value = false;
      return;
    }
    const request = secureCredentialClient.submit(id, secret.value, privateKey.value ? passphrase.value : "");
    clear();
    await request;
  } catch (error) {
    clear();
    showFailure(error);
    pending.value = false;
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
  try {
    prompt.value = await secureCredentialClient.get(id);
  } catch (error) {
    showFailure(error);
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
      >
        <NvxInput
          id="secure-credential-passphrase"
          v-model="passphrase"
          type="password"
          autocomplete="off"
          :placeholder="t('sshTerminal.passphrasePlaceholder')"
          :disabled="pending"
          :maxlength="65536"
        />
      </NvxField>
      <NvxInlineNotice
        v-if="failed"
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
