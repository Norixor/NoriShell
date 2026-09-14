<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { HOST_MARKER_COLORS, HOST_MARKER_PRESETS, normalizeHostMarker, type HostMarker, type HostMarkerColor, type HostMarkerPreset } from "../../host-markers";
import { NvxButton, NvxField, NvxInput, NvxSelect } from "../ui";
import NvxHostMarker from "./NvxHostMarker.vue";
const props = defineProps<{ modelValue: HostMarker | null; disabled?: boolean }>();
const emit = defineEmits<{ "update:modelValue": [value: HostMarker | null] }>();
const { t } = useI18n();
const options = computed(() => ["none", "production", "testing", "development", "custom"].map(value => ({ value, label: t(`hostMarkers.${value}`) })));
const colors = computed(() => HOST_MARKER_COLORS.map(value => ({ value, label: t(`hostMarkers.colors.${value}`) })));
function select(value: string) {
  emit("update:modelValue", value === "none" ? null : value === "custom" ? { kind: "custom", label: "", color: props.modelValue?.color ?? "blue" } : { ...HOST_MARKER_PRESETS[value as HostMarkerPreset] });
}
function setColor(color: string) { if (props.modelValue) emit("update:modelValue", { ...props.modelValue, color: color as HostMarkerColor }); }
function setLabel(label: string) { if (props.modelValue?.kind === "custom") emit("update:modelValue", { ...props.modelValue, label }); }
</script>
<template>
  <section class="nvx-host-marker-editor">
    <NvxField :label="t('hostMarkers.title')">
      <NvxSelect
        :model-value="modelValue?.kind ?? 'none'"
        :options="options"
        :aria-label="t('hostMarkers.title')"
        :disabled="disabled"
        @update:model-value="select"
      />
    </NvxField>
    <NvxField
      v-if="modelValue?.kind === 'custom'"
      :label="t('hostMarkers.label')"
    >
      <NvxInput
        :model-value="modelValue.label"
        :aria-label="t('hostMarkers.label')"
        :disabled="disabled"
        :invalid="!normalizeHostMarker(modelValue)"
        @update:model-value="setLabel"
      />
      <small v-if="!normalizeHostMarker(modelValue)">{{ t('hostMarkers.invalid') }}</small>
    </NvxField>
    <NvxField
      v-if="modelValue"
      :label="t('hostMarkers.color')"
    >
      <NvxSelect
        :model-value="modelValue.color"
        :options="colors"
        :aria-label="t('hostMarkers.color')"
        :disabled="disabled"
        @update:model-value="setColor"
      />
    </NvxField>
    <div
      v-if="modelValue"
      class="nvx-host-marker-editor__preview"
      :aria-label="t('hostMarkers.preview')"
    >
      <NvxHostMarker :marker="normalizeHostMarker(modelValue)" />
      <NvxButton
        variant="ghost"
        :disabled="disabled"
        @click="emit('update:modelValue', null)"
      >
        {{ t('hostMarkers.clear') }}
      </NvxButton>
    </div>
    <p>{{ t('hostMarkers.hint') }}</p>
  </section>
</template>
<style scoped>
.nvx-host-marker-editor { display: grid; gap: var(--nvx-space-2); min-width: 0; }
.nvx-host-marker-editor__preview { display: flex; flex-wrap: wrap; align-items: center; gap: var(--nvx-space-2); }
.nvx-host-marker-editor p, .nvx-host-marker-editor small { margin: 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
</style>
