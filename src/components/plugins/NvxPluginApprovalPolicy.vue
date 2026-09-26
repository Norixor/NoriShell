<script setup lang="ts">
import { computed, watch } from "vue";
import { useI18n } from "vue-i18n";

import { NvxField, NvxInlineNotice, NvxSelect } from "../ui";
import type {
  PluginApprovalExpiry,
  PluginApprovalPolicy,
  PluginRememberPolicy,
} from "../../core-api/generated/core-api";

const props = withDefaults(defineProps<{
  modelValue: PluginApprovalPolicy;
  modelExpiry: PluginApprovalExpiry;
  rememberPolicy: PluginRememberPolicy;
  risk: "remote" | "network" | "file" | "sftp" | "serial" | "local" | "host" | "terminal";
  part?: "details" | "choice";
  disabled?: boolean;
}>(), { part: "details" });

const emit = defineEmits<{
  "update:modelValue": [value: PluginApprovalPolicy];
  "update:modelExpiry": [value: PluginApprovalExpiry];
}>();
const { t } = useI18n();
const canRemember = computed(() => props.rememberPolicy === "exactOperation");
const expiryOptions = computed(() => [
  { value: "fifteenMinutes", label: t("plugins.approvalPolicy.expiry.fifteenMinutes") },
  { value: "oneHour", label: t("plugins.approvalPolicy.expiry.oneHour") },
  { value: "twentyFourHours", label: t("plugins.approvalPolicy.expiry.twentyFourHours") },
  { value: "unlimited", label: t("plugins.approvalPolicy.expiry.unlimited") },
]);
const rememberHint = computed(() => canRemember.value
  ? t("plugins.approvalPolicy.exactOperation")
  : t(`plugins.approvalPolicy.${props.rememberPolicy}`));

watch(canRemember, (available) => {
  if (!available && props.modelValue === "always") emit("update:modelValue", "once");
}, { immediate: true });
</script>

<template>
  <div
    v-if="part === 'details'"
    class="plugin-approval-policy"
  >
    <NvxInlineNotice
      tone="warning"
      :title="t(`plugins.approvalPolicy.risk.${risk}.title`)"
    >
      {{ t(`plugins.approvalPolicy.risk.${risk}.description`) }}
    </NvxInlineNotice>
    <NvxInlineNotice tone="info">
      {{ t("plugins.approvalPolicy.scopeNotice") }}
    </NvxInlineNotice>
  </div>
  <div
    v-else
    class="plugin-approval-choice"
  >
    <div class="plugin-approval-choice__heading">
      <strong>{{ t('plugins.approvalPolicy.label') }}</strong>
      <span>{{ rememberHint }}</span>
    </div>
    <div
      class="plugin-approval-choice__options"
      role="radiogroup"
      :aria-label="t('plugins.approvalPolicy.label')"
    >
      <label
        class="plugin-approval-choice__option"
        :class="{ 'plugin-approval-choice__option--selected': modelValue === 'once' }"
      >
        <input
          type="radio"
          name="plugin-approval-policy"
          value="once"
          :checked="modelValue === 'once'"
          :disabled="disabled"
          @change="emit('update:modelValue', 'once')"
        >
        {{ t('plugins.approvalPolicy.once') }}
      </label>
      <label
        class="plugin-approval-choice__option"
        :class="{ 'plugin-approval-choice__option--selected': modelValue === 'always' }"
      >
        <input
          type="radio"
          name="plugin-approval-policy"
          value="always"
          :checked="modelValue === 'always'"
          :disabled="disabled || !canRemember"
          @change="emit('update:modelValue', 'always')"
        >
        {{ t('plugins.approvalPolicy.always') }}
      </label>
    </div>
    <NvxField
      v-if="modelValue === 'always' && canRemember"
      :label="t('plugins.approvalPolicy.expiry.label')"
      :hint="t('plugins.approvalPolicy.expiry.hint')"
    >
      <NvxSelect
        :model-value="modelExpiry"
        :disabled="disabled"
        :options="expiryOptions"
        :aria-label="t('plugins.approvalPolicy.expiry.label')"
        @update:model-value="emit('update:modelExpiry', $event as PluginApprovalExpiry)"
      />
    </NvxField>
  </div>
</template>

<style scoped>
.plugin-approval-policy { display: grid; gap: var(--nvx-space-3); }
.plugin-approval-choice { display: grid; gap: var(--nvx-space-2); }
.plugin-approval-choice__heading { display: flex; align-items: baseline; justify-content: space-between; gap: var(--nvx-space-2); font-size: var(--nvx-font-size-xs); }
.plugin-approval-choice__heading span { color: var(--nvx-color-text-secondary); }
.plugin-approval-choice__options { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--nvx-space-2); }
.plugin-approval-choice__option { display: flex; align-items: center; gap: var(--nvx-space-2); min-width: 0; padding: var(--nvx-space-2); border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); cursor: pointer; }
.plugin-approval-choice__option--selected { border-color: var(--nvx-color-accent); background: var(--nvx-color-bg-surface); }
.plugin-approval-choice__option:has(input:disabled) { color: var(--nvx-color-text-tertiary); cursor: not-allowed; }
.plugin-approval-choice__option:focus-within { outline: var(--nvx-focus-ring-width) solid var(--nvx-color-accent); outline-offset: 2px; }
@media (max-width: 520px) { .plugin-approval-choice__heading { display: grid; } }
</style>
