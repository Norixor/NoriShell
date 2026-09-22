<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";
import { NvxButton, NvxCheckbox, NvxField, NvxInlineNotice, NvxInput } from "../components/ui";
import { secureVaultClient, type SecureVaultPrompt } from "../core-api/secure-vault-client";
const { t } = useI18n();
const id = new URLSearchParams(window.location.search).get("prompt") ?? "";
const prompt = ref<SecureVaultPrompt | null>(null);
const password = ref("");
const confirmation = ref("");
const confirmed = ref(false);
const pending = ref(false);
const failed = ref(false);
const creating = computed(() => prompt.value?.kind === "create");
const automatic = computed(() => prompt.value?.kind === "enableAutoUnlock");
const valid = computed(() => !!prompt.value && !!password.value && (!creating.value || (new TextEncoder().encode(password.value).length >= 12 && password.value === confirmation.value)) && (!automatic.value || confirmed.value));
const title = computed(() => t(automatic.value ? "sshSettings.vault.enableDialog.title" : creating.value ? "sshHosts.vault.createTitle" : "sshHosts.vault.unlockTitle"));
const description = computed(() => t(automatic.value ? "sshSettings.vault.enableDialog.description" : creating.value ? "plugins.sshSync.prompt.createLocalVault.description" : "sshHosts.vault.description"));
function clear() { password.value = ""; confirmation.value = ""; confirmed.value = false; }
async function submit() {
  if (pending.value || !valid.value) return;
  pending.value = true;
  failed.value = false;
  const request = secureVaultClient.submit(id, password.value, confirmation.value, confirmed.value);
  clear();
  try { await request; } catch { failed.value = true; pending.value = false; }
}
async function cancel() {
  if (pending.value) return;
  clear();
  pending.value = true;
  try { await secureVaultClient.cancel(id); } catch { failed.value = true; pending.value = false; }
}
onMounted(async () => { try { prompt.value = await secureVaultClient.get(id); } catch { failed.value = true; } });
onBeforeUnmount(clear);
</script>

<template>
  <NvxSecureWindow
    :title="title"
    :description="description"
    layout="form"
  >
    <form
      class="secure-vault-form"
      @submit.prevent="submit"
    >
      <NvxInlineNotice
        v-if="automatic"
        tone="warning"
      >
        {{ t("sshSettings.vault.enableDialog.securityNotice") }}
      </NvxInlineNotice>
      <NvxField
        for-id="secure-vault-password"
        :label="t('sshHosts.vault.password')"
      >
        <NvxInput
          id="secure-vault-password"
          v-model="password"
          type="password"
          :autocomplete="creating ? 'new-password' : 'current-password'"
          :disabled="pending || !prompt"
          autofocus
        />
      </NvxField>
      <NvxField
        v-if="creating"
        for-id="secure-vault-confirmation"
        :label="t('sshHosts.vault.passwordConfirmation')"
      >
        <NvxInput
          id="secure-vault-confirmation"
          v-model="confirmation"
          type="password"
          autocomplete="new-password"
          :disabled="pending"
        />
      </NvxField>
      <NvxCheckbox
        v-if="automatic"
        v-model="confirmed"
        :disabled="pending"
      >
        {{ t("sshSettings.vault.enableDialog.confirm") }}
      </NvxCheckbox>
      <NvxInlineNotice
        v-if="failed"
        tone="error"
      >
        {{ t("sshHosts.vault.failed") }}
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
        {{ t("sshSettings.vault.enableDialog.cancel") }}
      </NvxButton>
      <NvxButton
        :loading="pending"
        :disabled="!valid || pending"
        @click="submit"
      >
        {{ t(automatic ? "sshSettings.vault.enableDialog.enable" : creating ? "sshHosts.vault.createAction" : "sshHosts.vault.unlockAction") }}
      </NvxButton>
    </template>
  </NvxSecureWindow>
</template>

<style scoped>
.secure-vault-form { display: grid; gap: var(--nvx-space-4); }
</style>
