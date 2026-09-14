<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import { useI18n } from "vue-i18n";
import { NvxButton, NvxDialog, NvxField, NvxInlineNotice, NvxInput, NvxSelect } from "../ui";
import { cancelHostCreatePassword, createVault, fetchVaultStatus, stageHostCreatePassword, unlockVault } from "../../core-api/client";
import { createUuidV7 } from "../../core-api/ids";
import type { CredentialRefSummary, DesktopPasswordStage } from "../../core-api/generated/core-api";

const props = defineProps<{
  newProfile: boolean;
  modelValue: string | null;
  credentials: CredentialRefSummary[];
  label: string;
  busy: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: string | null] }>();
const { t } = useI18n();
const selection = ref(props.newProfile && !props.modelValue ? "new" : props.modelValue ?? "");
const password = ref("");
const passwordRequired = ref(false);
const staged = ref<DesktopPasswordStage | null>(null);
const failure = ref(false), working = ref(false);
const vaultMode = ref<"create" | "unlock" | null>(null);
const vaultPassword = ref(""), vaultConfirmation = ref(""), vaultSubmitting = ref(false), vaultFailed = ref(false);
let operation = freshOperation(), stageRequested = false, committed = false, disposed = false;
let finishVault: ((value: boolean) => void) | undefined;
const options = computed(() => [
  ...(props.newProfile ? [{ value: "new", label: t("desktop.enterPassword") }] : []),
  { value: "", label: t("desktop.ask") },
  ...props.credentials.filter((value) => value.method === "password").map((value) => ({ value: value.credentialRefId, label: value.label })),
  ...(props.modelValue && !props.credentials.some((value) => value.credentialRefId === props.modelValue)
    ? [{ value: props.modelValue, label: t("desktop.credential") }] : []),
]);
const vaultValid = computed(() => vaultPassword.value.length > 0 && (vaultMode.value !== "create"
  || (Array.from(vaultPassword.value).length >= 12 && new TextEncoder().encode(vaultPassword.value).length >= 12 && vaultPassword.value === vaultConfirmation.value)));
function freshOperation() { return { operationId: createUuidV7(), idempotencyKey: crypto.randomUUID() }; }
function clearVaultInputs() { vaultPassword.value = ""; vaultConfirmation.value = ""; }
function closeVault() {
  clearVaultInputs(); vaultMode.value = null;
  finishVault?.(false); finishVault = undefined;
}
async function cleanup() {
  if (stageRequested && !committed) await cancelHostCreatePassword(operation);
  if (!disposed) { staged.value = null; stageRequested = false; operation = freshOperation(); }
}
async function changeSelection(value: string) {
  if (props.busy || working.value) return;
  password.value = ""; failure.value = false; working.value = true;
  try {
    await cleanup();
    if (disposed) return;
    passwordRequired.value = false;
    selection.value = value;
    emit("update:modelValue", value === "new" || !value ? null : value);
  } catch { failure.value = true; } finally { working.value = false; }
}
async function ensureVault() {
  const status = await fetchVaultStatus();
  if (disposed) return false;
  if (status.state === "unlocked") return true;
  if (!["missing", "locked", "requiresReload"].includes(status.state)) throw new Error("Vault unavailable");
  clearVaultInputs(); vaultFailed.value = false;
  vaultMode.value = status.state === "missing" ? "create" : "unlock";
  return new Promise<boolean>((resolve) => { finishVault = resolve; });
}
async function submitVault() {
  if (!vaultValid.value || vaultSubmitting.value || !vaultMode.value) return;
  const mode = vaultMode.value;
  const value = vaultPassword.value, confirmation = vaultConfirmation.value;
  clearVaultInputs(); vaultSubmitting.value = true; vaultFailed.value = false;
  try {
    const status = mode === "create" ? await createVault(value, confirmation) : await unlockVault(value);
    if (disposed) return;
    if (status.state !== "unlocked") throw new Error("Vault unavailable");
    vaultMode.value = null;
    finishVault?.(true); finishVault = undefined;
  } catch { if (!disposed) vaultFailed.value = true; } finally { vaultSubmitting.value = false; }
}
async function prepare(): Promise<DesktopPasswordStage | null | false> {
  if (working.value || disposed) return false;
  failure.value = false;
  if (selection.value !== "new") return null;
  if (staged.value) return { ...staged.value };
  if (stageRequested) {
    working.value = true;
    try { await cleanup(); } catch { failure.value = true; return false; } finally { working.value = false; }
    if (disposed) return false;
  }
  if (!password.value) {
    if (passwordRequired.value) { failure.value = true; return false; }
    return null;
  }
  passwordRequired.value = true;
  if (new TextEncoder().encode(password.value).length > 4096 || password.value.includes("\0")) {
    password.value = ""; failure.value = true; return false;
  }
  working.value = true;
  try {
    if (!await ensureVault() || disposed) { password.value = ""; return false; }
    const value = password.value;
    password.value = "";
    stageRequested = true;
    const submittedOperation = { ...operation };
    const result = await stageHostCreatePassword({
      ...submittedOperation,
      identityLabel: props.label,
      credentialLabel: `${props.label} · ${t("desktop.password")}`,
      password: value,
    });
    if (disposed) { await cancelHostCreatePassword(submittedOperation); return false; }
    staged.value = { ...operation, stagedPasswordId: result.stagedPasswordId };
    return { ...staged.value };
  } catch {
    password.value = ""; failure.value = true;
    // Lost responses still clean up through the original operation; Core reconciles failures against its durable intent.
    try { await cleanup(); } catch { /* Retain the original operation and request cleanup again on unmount. */ }
    return false;
  } finally { working.value = false; }
}
function acceptSaved() { committed = true; staged.value = null; password.value = ""; }
onBeforeUnmount(() => {
  disposed = true; password.value = ""; closeVault();
  void cleanup().catch(() => { /* Core's expired-staging reconciliation retains cleanup responsibility. */ });
});
defineExpose({ prepare, acceptSaved });
</script>

<template>
  <NvxField :label="t('desktop.passwordSource')">
    <NvxSelect
      :model-value="selection"
      :options="options"
      :aria-label="t('desktop.passwordSource')"
      :disabled="busy || working"
      @update:model-value="changeSelection"
    />
  </NvxField>
  <NvxField
    v-if="selection === 'new'"
    for-id="desktop-password"
    :label="t('desktop.password')"
  >
    <NvxInput
      id="desktop-password"
      v-model="password"
      type="password"
      autocomplete="new-password"
      :maxlength="4096"
      :disabled="busy || working || !!staged"
      :placeholder="t(staged ? 'desktop.passwordStaged' : 'desktop.passwordPlaceholder')"
    />
  </NvxField>
  <p
    v-if="selection === 'new'"
    class="desktop-password-hint"
  >
    {{ t(staged ? 'desktop.passwordStaged' : 'desktop.passwordSaveHint') }}
  </p>
  <NvxButton
    v-if="staged"
    class="desktop-password-hint"
    variant="ghost"
    size="sm"
    :disabled="busy || working"
    @click="changeSelection('new')"
  >
    {{ t('desktop.reenterPassword') }}
  </NvxButton>
  <NvxInlineNotice
    v-if="failure"
    class="desktop-password-hint"
    tone="error"
    :title="t('desktop.passwordSaveFailed')"
  />
  <NvxDialog
    :model-value="!!vaultMode"
    plugin-protected
    :title="t(vaultMode === 'create' ? 'desktop.createVault' : 'desktop.unlockVault')"
    :description="t('desktop.passwordVaultHint')"
    :close-label="t('desktop.cancel')"
    :dismissible="!vaultSubmitting"
    @update:model-value="!$event && closeVault()"
  >
    <NvxField
      for-id="desktop-save-vault-password"
      :label="t('desktop.vaultPassword')"
    >
      <NvxInput
        id="desktop-save-vault-password"
        v-model="vaultPassword"
        type="password"
        :autocomplete="vaultMode === 'create' ? 'new-password' : 'current-password'"
        :disabled="vaultSubmitting"
      />
    </NvxField>
    <NvxField
      v-if="vaultMode === 'create'"
      for-id="desktop-save-vault-confirmation"
      :label="t('desktop.vaultPasswordConfirmation')"
    >
      <NvxInput
        id="desktop-save-vault-confirmation"
        v-model="vaultConfirmation"
        type="password"
        autocomplete="new-password"
        :disabled="vaultSubmitting"
      />
    </NvxField>
    <NvxInlineNotice
      v-if="vaultMode === 'create'"
      :title="t('desktop.vaultMinimum')"
    />
    <NvxInlineNotice
      v-if="vaultFailed"
      tone="error"
      :title="t('desktop.passwordVaultFailed')"
    />
    <template #actions>
      <NvxButton
        variant="ghost"
        :disabled="vaultSubmitting"
        @click="closeVault"
      >
        {{ t('desktop.cancel') }}
      </NvxButton>
      <NvxButton
        :loading="vaultSubmitting"
        :disabled="!vaultValid"
        @click="submitVault"
      >
        {{ t(vaultMode === 'create' ? 'desktop.createVault' : 'desktop.unlockVault') }}
      </NvxButton>
    </template>
  </NvxDialog>
</template>

<style scoped>
.desktop-password-hint { grid-column: 1 / -1; margin: 0; font-size: var(--nvx-font-size-xs); color: var(--nvx-color-text-secondary); }
</style>
