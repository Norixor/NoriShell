<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import type { DesktopPreferences } from "../../core-api/generated/core-api";
import { useDesktopPreferencesStore } from "../../stores/desktopPreferences";
import { useTipsStore } from "../../stores/tips";
import { NvxButton, NvxCheckbox, NvxField, NvxInlineNotice, NvxSelect } from "../ui";

const { t } = useI18n();
const preferences = useDesktopPreferencesStore();
const tips = useTipsStore();
const draft = ref<DesktopPreferences | null>(null);
const toggles = ["trayShowStatus", "trayShowHostNames", "notificationBackgroundOnly", "notifyTransferCompleted", "notifyTransferFailed", "notifyDisconnected"] as const;
const closeOptions = computed(() => (["hide", "quit"] as const).map((value) => ({ value, label: t(`desktopPreferences.close.${value}`) })));
const recentOptions = computed(() => Array.from({ length: 11 }, (_, value) => ({ value: String(value), label: value === 0 ? t("desktopPreferences.noRecent") : t("desktopPreferences.recentCount", { count: value }) })));
watch(() => preferences.snapshot, (snapshot) => {
  if (snapshot) draft.value = { ...snapshot.preferences };
}, { immediate: true });
const dirty = computed(() => draft.value && JSON.stringify(draft.value) !== JSON.stringify(preferences.snapshot?.preferences));

function setClose(value: string) {
  if (draft.value && (value === "hide" || value === "quit")) draft.value.windowCloseBehavior = value;
}
function setRecent(value: string) {
  const count = Number(value);
  if (draft.value && Number.isInteger(count) && count >= 0 && count <= 10) draft.value.trayRecentLimit = count;
}
async function save() {
  if (!draft.value) return;
  if (await preferences.replace({ ...draft.value })) tips.show({ scope: "desktop-preferences", tone: "success", title: t("desktopPreferences.saved") });
}
onMounted(() => { void preferences.refresh(); });
</script>

<template>
  <section
    class="desktop-preferences"
    aria-labelledby="desktop-preferences-title"
  >
    <header>
      <h2 id="desktop-preferences-title">
        {{ t('desktopPreferences.title') }}
      </h2>
      <p>{{ t('desktopPreferences.description') }}</p>
    </header>
    <NvxInlineNotice
      v-if="preferences.error"
      tone="error"
    >
      {{ t(`desktopPreferences.${preferences.error}`) }}
    </NvxInlineNotice>
    <NvxButton
      v-if="!draft || preferences.error"
      variant="secondary"
      :disabled="preferences.busy"
      @click="preferences.refresh()"
    >
      {{ t('desktopPreferences.reload') }}
    </NvxButton>
    <p
      v-if="!draft && preferences.busy"
      role="status"
    >
      {{ t('desktopPreferences.loading') }}
    </p>
    <template v-if="draft">
      <NvxField :label="t('desktopPreferences.windowCloseBehavior')">
        <NvxSelect
          :model-value="draft.windowCloseBehavior"
          :options="closeOptions"
          :disabled="preferences.busy"
          :aria-label="t('desktopPreferences.windowCloseBehavior')"
          @update:model-value="setClose"
        />
      </NvxField>
      <p>{{ t('desktopPreferences.closeHint') }}</p>
      <NvxField :label="t('desktopPreferences.trayRecentLimit')">
        <NvxSelect
          :model-value="String(draft.trayRecentLimit)"
          :options="recentOptions"
          :disabled="preferences.busy"
          :aria-label="t('desktopPreferences.trayRecentLimit')"
          @update:model-value="setRecent"
        />
      </NvxField>
      <NvxCheckbox
        v-for="key in toggles"
        :key="key"
        v-model="draft[key]"
        :disabled="preferences.busy"
      >
        {{ t(`desktopPreferences.${key}`) }}
      </NvxCheckbox>
      <p>{{ t('desktopPreferences.notificationsHint') }}</p>
      <div class="desktop-preferences__actions">
        <NvxButton
          :disabled="preferences.busy || !dirty"
          @click="save"
        >
          {{ t('desktopPreferences.save') }}
        </NvxButton>
        <NvxButton
          variant="secondary"
          :disabled="preferences.busy || !dirty"
          @click="draft = preferences.snapshot ? { ...preferences.snapshot.preferences } : null"
        >
          {{ t('desktopPreferences.discard') }}
        </NvxButton>
      </div>
    </template>
  </section>
</template>

<style scoped>
.desktop-preferences { display: grid; gap: var(--nvx-space-3); max-width: 780px; }
.desktop-preferences h2 { margin: 0; font-size: var(--nvx-font-size-lg); }
.desktop-preferences p { margin: 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); line-height: 1.5; }
.desktop-preferences__actions { display: flex; gap: var(--nvx-space-2); }
</style>
