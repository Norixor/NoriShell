<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useSftpPreferencesStore, type SftpBrowserPreferences } from "../../stores/sftpPreferences";
import { applicationPreferenceFailure } from "../../core-api/application-preferences";
import { useTipsStore } from "../../stores/tips";
import { NvxCheckbox, NvxField, NvxSelect } from "../ui";

const { t } = useI18n();
const preferences = useSftpPreferencesStore();
const tips = useTipsStore();
const sorts = computed(() => (["name", "size", "modified"] as const).map((value) => ({ value, label: t(`sftp.sorts.${value}`) })));
async function save(patch: Partial<SftpBrowserPreferences>) {
  try { report(await preferences.replaceBrowser({ ...preferences.browser, ...patch })); }
  catch (error) { reportError(error); }
}
function report(ok: boolean) {
  tips.show({ scope: "sftp-preferences", tone: !ok || preferences.directoryMemoryPersistenceFailed ? "error" : "success",
    title: t(!ok ? "sftpSettings.saveFailed" : preferences.directoryMemoryPersistenceFailed ? "sftpSettings.globalSavedLocalMemoryFailed" : "sftpSettings.saved") });
}
function reportError(error: unknown) {
  tips.show({ scope: "sftp-preferences", tone: "error", title: t(`applicationPreferenceErrors.${applicationPreferenceFailure(error)}`) });
}
function setSort(sort: string) {
  if (sort === "name" || sort === "size" || sort === "modified") save({ sort });
}
async function setRememberLastDirectory(value: boolean) {
  try { report(await preferences.setRememberLastDirectory(value)); }
  catch (error) { reportError(error); }
}
</script>

<template>
  <section
    class="sftp-settings"
    aria-labelledby="sftp-settings-title"
  >
    <header>
      <h2 id="sftp-settings-title">
        {{ t('sftpSettings.title') }}
      </h2>
      <p>{{ t('sftpSettings.description') }}</p>
    </header>
    <NvxCheckbox
      :model-value="preferences.browser.showHidden"
      @update:model-value="save({ showHidden: $event })"
    >
      {{ t('sftpSettings.showHidden') }}
      <template #hint>
        {{ t('sftpSettings.hiddenHint') }}
      </template>
    </NvxCheckbox>
    <NvxCheckbox
      :model-value="preferences.browser.foldersFirst"
      @update:model-value="save({ foldersFirst: $event })"
    >
      {{ t('sftpSettings.foldersFirst') }}
    </NvxCheckbox>
    <NvxCheckbox
      :model-value="preferences.rememberLastDirectory"
      @update:model-value="setRememberLastDirectory"
    >
      {{ t('sftpSettings.rememberLastDirectory') }}
      <template #hint>
        {{ t('sftpSettings.rememberLastDirectoryHint') }}
      </template>
    </NvxCheckbox>
    <NvxField :label="t('sftpSettings.sort')">
      <NvxSelect
        :model-value="preferences.browser.sort"
        :options="sorts"
        :aria-label="t('sftpSettings.sort')"
        @update:model-value="setSort"
      />
    </NvxField>
  </section>
</template>

<style scoped>
.sftp-settings { display: grid; gap: var(--nvx-space-3); max-width: 780px; }
.sftp-settings h2 { margin: 0; font-size: var(--nvx-font-size-lg); }
.sftp-settings p { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); line-height: 1.5; }
</style>
