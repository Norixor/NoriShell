<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { NvxButton, NvxField, NvxInlineNotice, NvxInput, NvxSelect } from "../ui";
import { cancelHostCreatePassword, stageHostCreatePassword } from "../../core-api/client";
import { createUuidV7 } from "../../core-api/ids";
import { requestSecureVault } from "../../core-api/secure-vault-client";
import type { CredentialRefSummary, DesktopPasswordStage } from "../../core-api/generated/core-api";

const props = defineProps<{
  newProfile: boolean;
  modelValue: string | null;
  credentials: CredentialRefSummary[];
  label: string;
  busy: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: string | null]; "dirty-change": [value: boolean] }>();
const { t } = useI18n();
const selection = ref(props.newProfile && !props.modelValue ? "new" : props.modelValue ?? "");
const password = ref("");
const passwordRequired = ref(false);
const staged = ref<DesktopPasswordStage | null>(null);
const failure = ref(false), working = ref(false), stageRequested = ref(false);
let operation = freshOperation(), committed = false, disposed = false;
const options = computed(() => [
  ...(props.newProfile ? [{ value: "new", label: t("desktop.enterPassword") }] : []),
  { value: "", label: t("desktop.ask") },
  ...props.credentials.filter((value) => value.method === "password").map((value) => ({ value: value.credentialRefId, label: value.label })),
  ...(props.modelValue && !props.credentials.some((value) => value.credentialRefId === props.modelValue)
    ? [{ value: props.modelValue, label: t("desktop.credential") }] : []),
]);
const dirty = computed(() => selection.value === "new" && (password.value.length > 0 || passwordRequired.value || !!staged.value || stageRequested.value));
function freshOperation() { return { operationId: createUuidV7(), idempotencyKey: crypto.randomUUID() }; }
async function cleanup() {
  if (stageRequested.value && !committed) await cancelHostCreatePassword(operation);
  if (!disposed) { staged.value = null; stageRequested.value = false; operation = freshOperation(); }
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
  const unlocked = await requestSecureVault("ensureUnlocked");
  return !disposed && unlocked;
}
async function prepare(): Promise<DesktopPasswordStage | null | false> {
  if (working.value || disposed) return false;
  failure.value = false;
  if (selection.value !== "new") return null;
  if (staged.value) return { ...staged.value };
  if (stageRequested.value) {
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
    stageRequested.value = true;
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
function acceptSaved() { committed = true; staged.value = null; stageRequested.value = false; passwordRequired.value = false; password.value = ""; }
async function discard(): Promise<boolean> {
  if (working.value || disposed) return false;
  password.value = "";
  working.value = true;
  try {
    await cleanup();
    passwordRequired.value = false;
    failure.value = false;
    return true;
  } catch {
    failure.value = true;
    return false;
  } finally { working.value = false; }
}
watch(dirty, (value) => emit("dirty-change", value), { immediate: true });
onBeforeUnmount(() => {
  disposed = true; password.value = "";
  void cleanup().catch(() => { /* Core's expired-staging reconciliation retains cleanup responsibility. */ });
});
defineExpose({ prepare, acceptSaved, discard });
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
</template>

<style scoped>
.desktop-password-hint { grid-column: 1 / -1; margin: 0; font-size: var(--nvx-font-size-xs); color: var(--nvx-color-text-secondary); }
</style>
