<script setup lang="ts">
import { computed, watch } from "vue";
import { useI18n } from "vue-i18n";

import { NvxField, NvxInlineNotice, NvxSelect } from "../ui";
import type {
  PluginApprovalExpiry,
  PluginApprovalPolicy,
  PluginRememberPolicy,
} from "../../core-api/generated/core-api";

const props = defineProps<{
  modelValue: PluginApprovalPolicy;
  modelExpiry: PluginApprovalExpiry;
  rememberPolicy: PluginRememberPolicy;
  risk: "remote" | "network" | "file" | "sftp" | "serial" | "local" | "host" | "terminal";
  disabled?: boolean;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: PluginApprovalPolicy];
  "update:modelExpiry": [value: PluginApprovalExpiry];
}>();
const { t } = useI18n();
const canRemember = computed(() => props.rememberPolicy === "exactOperation");
const options = computed(() => [
  { value: "once", label: t("plugins.approvalPolicy.once") },
  {
    value: "always",
    label: t("plugins.approvalPolicy.always"),
    disabled: !canRemember.value,
  },
]);
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
  <div class="plugin-approval-policy">
    <NvxInlineNotice
      tone="warning"
      :title="t(`plugins.approvalPolicy.risk.${risk}.title`)"
    >
      {{ t(`plugins.approvalPolicy.risk.${risk}.description`) }}
    </NvxInlineNotice>
    <NvxField
      :label="t('plugins.approvalPolicy.label')"
      :hint="rememberHint"
    >
      <NvxSelect
        :model-value="modelValue"
        :disabled="disabled"
        :options="options"
        :aria-label="t('plugins.approvalPolicy.label')"
        @update:model-value="emit('update:modelValue', $event as PluginApprovalPolicy)"
      />
    </NvxField>
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
    <NvxInlineNotice tone="info">
      {{ t("plugins.approvalPolicy.scopeNotice") }}
    </NvxInlineNotice>
  </div>
</template>

<style scoped>
.plugin-approval-policy { display: grid; gap: var(--nvx-space-3); }
</style>
