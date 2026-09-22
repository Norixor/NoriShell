<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";

import { useHostMarkersStore } from "../../stores/hostMarkers";
import { useTerminalPreferencesStore, type PasteWarningMode } from "../../stores/terminalPreferences";
import { useTipsStore } from "../../stores/tips";
import { NvxSelect } from "../ui";

const { t } = useI18n();
const markers = useHostMarkersStore();
const preferences = useTerminalPreferencesStore();
const tips = useTipsStore();
const pasteOptions = computed(() => (["multiline", "always", "never"] as const).map((value) => ({ value, label: t(`terminalEnhancements.paste.${value}`) })));
const markerOptions = computed(() => [
  { value: "show", label: t("terminalEnhancements.show") },
  { value: "hide", label: t("terminalEnhancements.hide") },
]);
function report(ok: boolean) {
  tips.show({ scope: "terminal-enhancement-settings", tone: ok ? "success" : "error", title: t(`terminalEnhancements.${ok ? "saved" : "saveFailed"}`) });
}
function changePaste(value: string) { report(preferences.setPasteWarning(value as PasteWarningMode)); }
function changeMarkers(value: string) { report(markers.setEnabled(value === "show") === "saved"); }
</script>

<template>
  <section
    class="enhancement-settings"
    aria-labelledby="terminal-enhancement-title"
  >
    <header>
      <h2 id="terminal-enhancement-title">
        {{ t('terminalEnhancements.title') }}
      </h2>
      <p>{{ t('terminalEnhancements.description') }}</p>
    </header>
    <div class="enhancement-settings__rows">
      <div class="enhancement-settings__row">
        <span><strong>{{ t('terminalEnhancements.paste.setting') }}</strong><small>{{ t('terminalEnhancements.paste.settingHint') }}</small></span>
        <NvxSelect
          :model-value="preferences.preferences.pasteWarning"
          :options="pasteOptions"
          :aria-label="t('terminalEnhancements.paste.setting')"
          @update:model-value="changePaste"
        />
      </div>
      <div class="enhancement-settings__row">
        <span><strong>{{ t('terminalEnhancements.hostMarkers') }}</strong><small>{{ t('terminalEnhancements.hostMarkersHint') }}</small></span>
        <NvxSelect
          :model-value="markers.enabled ? 'show' : 'hide'"
          :options="markerOptions"
          :aria-label="t('terminalEnhancements.hostMarkers')"
          @update:model-value="changeMarkers"
        />
      </div>
    </div>
    <slot />
  </section>
</template>

<style scoped>
.enhancement-settings { display: grid; gap: var(--nvx-space-5); min-width: 0; }
.enhancement-settings h2 { margin: 0; font-size: var(--nvx-font-size-lg); }
.enhancement-settings p { margin: 5px 0 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.enhancement-settings__rows { display: grid; }
.enhancement-settings__row { display: grid; grid-template-columns: minmax(0, 1fr) minmax(180px, 260px); gap: var(--nvx-space-4); align-items: center; padding: var(--nvx-space-3) 0; border-bottom: 1px solid var(--nvx-color-border-subtle); }
.enhancement-settings__row strong { display: block; font-size: var(--nvx-font-size-sm); font-weight: 500; }
.enhancement-settings__row small { display: block; margin-top: 3px; color: var(--nvx-color-text-secondary); line-height: 1.5; }
@media (max-width: 760px) { .enhancement-settings__row { grid-template-columns: minmax(0, 1fr); gap: var(--nvx-space-2); } }
</style>
