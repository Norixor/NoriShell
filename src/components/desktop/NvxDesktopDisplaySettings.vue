<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { DesktopProfile } from "../../core-api/generated/core-api";
import { NvxField, NvxInput, NvxSelect } from "../ui";
const props = withDefaults(defineProps<{ modelValue: DesktopProfile; idPrefix?: string }>(), { idPrefix: "desktop" });
const emit = defineEmits<{ "update:modelValue": [profile: DesktopProfile] }>();
const { t } = useI18n();
const presets = ["1280x720", "1280x800", "1600x900", "1920x1080", "1920x1200", "2560x1440", "3840x2160"];
const custom = ref(false);
const resolution = computed(() => {
  const value = `${props.modelValue.width}x${props.modelValue.height}`;
  return !custom.value && presets.includes(value) ? value : "custom";
});
const adaptiveMode = computed(() => props.modelValue.protocol === "rdp"
  ? props.modelValue.rdpResolutionMode === "adaptive"
  : props.modelValue.vncResolutionMode === "adaptive");
const serverSize = computed(() => props.modelValue.protocol === "vnc" && props.modelValue.vncResolutionMode === "server");
const resolutionHint = computed(() => {
  if (props.modelValue.protocol === "rdp") return adaptiveMode.value ? "adaptiveResolutionHint" : "resolutionHint";
  if (serverSize.value) return "vncResolutionHint";
  return adaptiveMode.value ? "vncAdaptiveResolutionHint" : "vncFixedResolutionHint";
});
function update(value: Partial<DesktopProfile>) { emit("update:modelValue", { ...props.modelValue, ...value }); }
function setResolution(value: string) {
  custom.value = value === "custom";
  if (!presets.includes(value)) return;
  const [width, height] = value.split("x").map(Number);
  update({ width, height });
}
function setResolutionMode(value: string) {
  if (value === "fixed" || value === "adaptive") update({ rdpResolutionMode: value });
}
function setVncResolutionMode(value: string) {
  if (value === "server" || value === "fixed" || value === "adaptive") update({ vncResolutionMode: value });
}
function setTransport(value: string) {
  if (value === "auto" || value === "tcpOnly" || value === "udpRequired") update({ rdpTransportMode: value });
}
function setGraphics(value: string) {
  if (value === "auto" || value === "remoteFx" || value === "avc420" || value === "bitmap") update({ rdpGraphicsMode: value });
}
</script>
<template>
  <div class="desktop-display-settings">
    <template v-if="modelValue.protocol === 'rdp'">
      <NvxField :label="t('desktop.transportMode')">
        <NvxSelect
          :model-value="modelValue.rdpTransportMode ?? 'auto'"
          :aria-label="t('desktop.transportMode')"
          :options="[
            { value: 'auto', label: t('desktop.transportModes.auto') },
            { value: 'tcpOnly', label: t('desktop.transportModes.tcpOnly') },
            { value: 'udpRequired', label: t('desktop.transportModes.udpRequired'), disabled: !!modelValue.gatewayHostId },
          ]"
          @update:model-value="setTransport"
        />
      </NvxField>
      <NvxField :label="t('desktop.graphicsMode')">
        <NvxSelect
          :model-value="modelValue.rdpGraphicsMode ?? 'auto'"
          :aria-label="t('desktop.graphicsMode')"
          :options="[
            { value: 'auto', label: t('desktop.graphicsModes.auto') },
            { value: 'remoteFx', label: t('desktop.graphicsModes.remoteFx') },
            { value: 'avc420', label: t('desktop.graphicsModes.avc420') },
            { value: 'bitmap', label: t('desktop.graphicsModes.bitmap') },
          ]"
          @update:model-value="setGraphics"
        />
      </NvxField>
      <p class="desktop-display-settings__hint">
        {{ t(modelValue.gatewayHostId ? 'desktop.transportGatewayHint' : 'desktop.transportHint') }}
      </p>
      <p class="desktop-display-settings__hint">
        {{ t('desktop.graphicsHint') }}
      </p>
    </template>
    <NvxField
      v-if="modelValue.protocol === 'rdp'"
      class="desktop-display-settings__wide"
      :label="t('desktop.resolutionMode')"
    >
      <NvxSelect
        :model-value="modelValue.rdpResolutionMode ?? 'fixed'"
        :aria-label="t('desktop.resolutionMode')"
        :options="[{ value: 'fixed', label: t('desktop.resolutionModes.fixed') }, { value: 'adaptive', label: t('desktop.resolutionModes.adaptive') }]"
        @update:model-value="setResolutionMode"
      />
    </NvxField>
    <NvxField
      v-else
      class="desktop-display-settings__wide"
      :label="t('desktop.vncResolutionMode')"
    >
      <NvxSelect
        :model-value="modelValue.vncResolutionMode ?? 'server'"
        :aria-label="t('desktop.vncResolutionMode')"
        :options="[{ value: 'server', label: t('desktop.vncResolutionModes.server') }, { value: 'fixed', label: t('desktop.vncResolutionModes.fixed') }, { value: 'adaptive', label: t('desktop.vncResolutionModes.adaptive') }]"
        @update:model-value="setVncResolutionMode"
      />
    </NvxField>
    <NvxField
      class="desktop-display-settings__wide"
      :label="t(adaptiveMode ? 'desktop.initialResolution' : 'desktop.resolution')"
    >
      <NvxSelect
        :disabled="serverSize"
        :model-value="resolution"
        :aria-label="t('desktop.resolution')"
        :options="[...presets.map(value => ({ value, label: value.replace('x', ' × ') })), { value: 'custom', label: t('desktop.customResolution') }]"
        @update:model-value="setResolution"
      />
    </NvxField>
    <NvxField
      v-for="dimension in (['width', 'height'] as const)"
      v-show="resolution === 'custom'"
      :key="dimension"
      :for-id="`${idPrefix}-${dimension}`"
      :label="t(`desktop.${dimension}`)"
    >
      <NvxInput
        :id="`${idPrefix}-${dimension}`"
        :disabled="serverSize"
        :model-value="String(modelValue[dimension])"
        type="number"
        :min="200"
        :max="8192"
        @update:model-value="update({ [dimension]: Number($event) })"
      />
    </NvxField>
    <p class="desktop-display-settings__hint desktop-display-settings__wide">
      {{ t(`desktop.${resolutionHint}`) }}
    </p>
  </div>
</template>
<style scoped>
.desktop-display-settings { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--nvx-space-3); }
.desktop-display-settings__wide { grid-column: 1 / -1; }
.desktop-display-settings__hint { margin: 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-sm); }
@media (max-width: 560px) { .desktop-display-settings { grid-template-columns: 1fr; } }
</style>
