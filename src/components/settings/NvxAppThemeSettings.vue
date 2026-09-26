<script setup lang="ts">
import { Check, Download, Palette, RefreshCw, Upload } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import { themeStyles, themeWithOverride, validateAppThemeProfile, validateThemeDefinition, type AppThemeProfile, type ThemeDefinition } from "../../app-theme";
import { exportJsonFile } from "../../platform-file-export";
import { useAppThemeStore } from "../../stores/appTheme";
import { applicationPreferenceFailure } from "../../core-api/application-preferences";
import { useUiStore, type ThemePreference } from "../../stores/ui";
import { NvxButton, NvxField, NvxIcon, NvxInlineNotice, NvxInput, NvxSelect, NvxStatusLabel } from "../ui";

const { t, locale } = useI18n();
const router = useRouter();
const store = useAppThemeStore();
const ui = useUiStore();
const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
const draft = ref<AppThemeProfile>(clone(store.profile));
const baseline = ref<AppThemeProfile>(clone(store.profile));
const previewAppearance = ref<"light" | "dark">(ui.theme);
const pendingMode = ref<ThemePreference | null>(null);
const previewInput = ref("");
const previewTab = ref("components");
const fileInput = ref<HTMLInputElement>();
const error = ref("");
const message = ref("");
const busy = ref(false);
let alive = true;
const selectedKey = computed(() => previewAppearance.value === "light" ? draft.value.lightThemeId : draft.value.darkThemeId);
const choices = computed(() => [...store.themes].sort((a, b) => Number(b.source === "plugin") - Number(a.source === "plugin")));
const selected = computed(() => choices.value.find((item) => item.key === selectedKey.value));
const dirty = computed(() => JSON.stringify(draft.value) !== JSON.stringify(baseline.value)
  || (pendingMode.value !== null && pendingMode.value !== ui.themePreference));
const missing = computed(() => !selected.value?.enabled);
const baseDefinition = computed(() => selected.value?.enabled ? selected.value.definition : store.resolveTheme(previewAppearance.value));
const editedDefinition = computed<ThemeDefinition>(() => {
  const base = baseDefinition.value;
  const override = draft.value.overrides[selectedKey.value] ?? {};
  return { ...clone(base), ...override, colors: { ...base.colors, ...override.colors } };
});
const valid = computed(() => validateAppThemeProfile(draft.value)
  && validateThemeDefinition(editedDefinition.value)
  && (["light", "dark"] as const).every((appearance) => {
    const key = appearance === "light" ? draft.value.lightThemeId : draft.value.darkThemeId;
    const choice = store.themes.find((item) => item.key === key && item.enabled && item.definition.appearance === appearance);
    return !choice || validateThemeDefinition(themeWithOverride(choice.definition, draft.value.overrides[key]));
  }));
const previewStyle = computed(() => themeStyles(valid.value ? editedDefinition.value : baseDefinition.value));
const fields = ["fontFamily", "fontSize", "radius", "borderWidth", "density", "shadow"] as const;
const colorGroups = [
  { id: "surfaces", keys: ["bgCanvas", "bgSurface", "bgSubtle", "bgHover", "border", "borderStrong"] },
  { id: "text", keys: ["textPrimary", "textSecondary", "textTertiary", "selection", "selectionText"] },
  { id: "actions", keys: ["accent", "accentHover", "onAccent", "accentSoft", "focusRing"] },
  { id: "status", keys: ["success", "successSoft", "warning", "warningSoft", "danger", "onDanger", "dangerSoft"] },
  { id: "brand", keys: ["brandMarkPrimary", "brandMarkSecondary", "terminalPaneActiveBorder"] },
] as const;
const modeOptions = computed(() => ["light", "dark", "system"].map((value) => ({ value, label: t(`appTheme.${value}`) })));
function fieldOptions(field: typeof fields[number]) {
  if (field === "fontSize") return [12, 13, 14, 15, 16, 17, 18].map((v) => ({ value: String(v), label: `${v}px` }));
  if (field === "radius") return [0, 2, 4, 6, 8, 10, 12].map((v) => ({ value: String(v), label: `${v}px` }));
  if (field === "borderWidth") return [1, 2].map((v) => ({ value: String(v), label: `${v}px` }));
  const values = field === "fontFamily" ? ["system", "sans", "mono"] : field === "density" ? ["compact", "standard", "comfortable"] : ["none", "soft", "standard"];
  const group = field === "fontFamily" ? "fonts" : field === "density" ? "densities" : "shadows";
  return values.map((value) => ({ value, label: t(`appTheme.${group}.${value}`) }));
}
function choose(key: string) {
  const choice = choices.value.find((item) => item.key === key && item.enabled);
  if (!choice) return;
  previewAppearance.value = choice.definition.appearance;
  pendingMode.value = choice.definition.appearance;
  if (previewAppearance.value === "light") draft.value.lightThemeId = key;
  else draft.value.darkThemeId = key;
  error.value = ""; message.value = "";
}
function setField(field: typeof fields[number], value: string) {
  const parsed = ["fontSize", "radius", "borderWidth"].includes(field) ? Number(value) : value;
  draft.value.overrides[selectedKey.value] = { ...draft.value.overrides[selectedKey.value], [field]: parsed };
  error.value = ""; message.value = "";
}
function setColor(key: typeof colorGroups[number]["keys"][number], value: string) {
  const previous = draft.value.overrides[selectedKey.value];
  draft.value.overrides[selectedKey.value] = { ...previous, colors: { ...previous?.colors, [key]: value } };
  error.value = ""; message.value = "";
}
function reset() {
  delete draft.value.overrides[selectedKey.value];
  message.value = "resetHint"; error.value = "";
}
function discard() {
  pendingMode.value = null;
  previewAppearance.value = ui.theme;
  draft.value = clone(store.profile); baseline.value = clone(store.profile);
  error.value = ""; message.value = "";
}
async function save() {
  if (!valid.value) { error.value = "invalid"; return; }
  try {
    if (!await store.saveProfile(clone(draft.value), baseline.value)) { error.value = "saveFailed"; return; }
    draft.value = clone(store.profile); baseline.value = clone(store.profile);
    // Keep the saved profile as the retry baseline if mode persistence fails.
    if (pendingMode.value !== null && !await ui.setThemePreference(pendingMode.value)) { error.value = "modeSaveFailed"; return; }
    pendingMode.value = null;
    message.value = "saved"; error.value = "";
  } catch (failure) { error.value = `preference:${applicationPreferenceFailure(failure)}`; }
}
async function changeMode(value: string) {
  try {
    if (!await ui.setThemePreference(value as ThemePreference)) { error.value = "saveFailed"; return; }
    pendingMode.value = null;
    previewAppearance.value = ui.theme;
  } catch (failure) { error.value = `preference:${applicationPreferenceFailure(failure)}`; }
}
async function readFile(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0]; input.value = "";
  if (!file || busy.value) return;
  busy.value = true; error.value = ""; message.value = "";
  try {
    if (file.size > 32 * 1024) { error.value = "tooLarge"; return; }
    const parsed: unknown = JSON.parse(await file.text());
    if (!alive) return;
    if (!validateAppThemeProfile(parsed)) { error.value = "invalidFile"; return; }
    draft.value = clone(parsed); message.value = "imported";
  } catch { if (alive) error.value = "invalidFile"; }
  finally { if (alive) busy.value = false; }
}
async function exportFile() {
  if (busy.value) return;
  busy.value = true; error.value = ""; message.value = "";
  try {
    if (await exportJsonFile("theme", store.exportProfile()) && alive) message.value = "exported";
  } catch { if (alive) error.value = "exportFailed"; }
  finally { if (alive) busy.value = false; }
}
watch(() => store.profile, (profile) => {
  if (!dirty.value) { draft.value = clone(profile); baseline.value = clone(profile); }
});
onMounted(() => { void store.refreshThemes(); });
onBeforeUnmount(() => { alive = false; });
</script>

<template>
  <section
    class="theme-settings"
    aria-labelledby="app-theme-title"
  >
    <header class="theme-settings__heading">
      <div>
        <h2 id="app-theme-title">
          {{ t('appTheme.title') }}
        </h2><p>{{ t('appTheme.description') }}</p>
      </div>
      <NvxButton
        variant="ghost"
        size="sm"
        :disabled="store.loading"
        @click="store.refreshThemes()"
      >
        <NvxIcon
          :icon="RefreshCw"
          :size="16"
        />{{ t('appTheme.refresh') }}
      </NvxButton>
    </header>
    <div class="theme-settings__mode">
      <div><strong>{{ t('appTheme.mode') }}</strong><p>{{ t('appTheme.modeHint') }}</p></div>
      <NvxSelect
        :model-value="ui.themePreference"
        :options="modeOptions"
        :aria-label="t('appTheme.mode')"
        @update:model-value="changeMode"
      />
    </div>
    <NvxInlineNotice
      v-if="store.loadError"
      tone="warning"
    >
      {{ t('appTheme.loadFailed') }}
    </NvxInlineNotice>
    <p
      v-if="store.loading"
      role="status"
    >
      {{ t('appTheme.loading') }}
    </p>
    <div class="theme-settings__workspace">
      <div class="theme-settings__editor">
        <div class="theme-settings__section-heading">
          <h3>{{ t('appTheme.theme') }}</h3><NvxButton
            variant="ghost"
            size="sm"
            @click="router.push('/plugins')"
          >
            {{ t('appTheme.manage') }}
          </NvxButton>
        </div>
        <p class="theme-settings__hint">
          {{ t('appTheme.selectionHint') }}
        </p>
        <div
          class="theme-settings__choices"
          role="group"
          :aria-label="t('appTheme.theme')"
        >
          <button
            v-for="choice in choices"
            :key="choice.key"
            type="button"
            class="theme-choice"
            :class="{ 'theme-choice--selected': selectedKey === choice.key }"
            :disabled="!choice.enabled"
            :aria-pressed="selectedKey === choice.key"
            @click="choose(choice.key)"
          >
            <span
              class="theme-choice__swatches"
              aria-hidden="true"
            ><span
              v-for="key in (['bgCanvas', 'bgSurface', 'accent', 'textPrimary'] as const)"
              :key="key"
              :style="{ backgroundColor: choice.definition.colors[key] }"
            /></span>
            <span class="theme-choice__name">{{ locale === 'zh-CN' ? choice.definition.name.zhCN : choice.definition.name.en }}<NvxIcon
              v-if="selectedKey === choice.key"
              :icon="Check"
              :size="16"
            /></span>
            <small>{{ t(`appTheme.${choice.definition.appearance}`) }} · {{ t(!choice.enabled ? 'appTheme.disabled' : choice.source === 'builtin' ? 'appTheme.builtin' : 'appTheme.installed') }}</small>
          </button>
        </div>
        <p
          v-if="!choices.some(item => item.source === 'plugin')"
          class="theme-settings__hint"
        >
          {{ t('appTheme.empty') }}
        </p>
        <NvxInlineNotice
          v-if="missing"
          tone="warning"
        >
          {{ t('appTheme.missing') }}
        </NvxInlineNotice>
        <div class="theme-settings__section-heading">
          <h3>{{ t('appTheme.customize') }}</h3><NvxButton
            variant="ghost"
            size="sm"
            :disabled="missing"
            @click="reset"
          >
            {{ t('appTheme.reset') }}
          </NvxButton>
        </div>
        <div class="theme-settings__fields">
          <NvxField
            v-for="field in fields"
            :key="field"
            :label="t(`appTheme.${field}`)"
          >
            <NvxSelect
              :model-value="String(editedDefinition[field])"
              :options="fieldOptions(field)"
              :aria-label="t(`appTheme.${field}`)"
              :disabled="missing"
              @update:model-value="setField(field, $event)"
            />
          </NvxField>
        </div>
        <details class="theme-settings__colors">
          <summary>
            <NvxIcon
              :icon="Palette"
              :size="16"
            />{{ t('appTheme.colors') }}
          </summary>
          <p>{{ t('appTheme.colorHint') }}</p>
          <fieldset
            v-for="group in colorGroups"
            :key="group.id"
          >
            <legend>{{ t(`appTheme.groups.${group.id}`) }}</legend>
            <div
              v-for="key in group.keys"
              :key="key"
              class="theme-color"
            >
              <label :for="`theme-color-${key}`">{{ t(`appTheme.tokens.${key}`) }}</label>
              <input
                type="color"
                :aria-label="t(`appTheme.tokens.${key}`)"
                :value="/^#[0-9a-fA-F]{6}$/.test(editedDefinition.colors[key]) ? editedDefinition.colors[key] : '#000000'"
                :disabled="missing"
                @input="setColor(key, ($event.target as HTMLInputElement).value)"
              >
              <NvxInput
                :id="`theme-color-${key}`"
                :model-value="editedDefinition.colors[key]"
                :maxlength="7"
                :disabled="missing"
                @update:model-value="setColor(key, $event)"
              />
            </div>
          </fieldset>
        </details>
      </div>
      <aside class="theme-settings__preview-area">
        <h3>{{ t('appTheme.preview') }}</h3><p class="theme-settings__hint">
          {{ t('appTheme.previewHint') }}
        </p>
        <div
          class="theme-preview"
          :style="previewStyle"
          :data-theme="previewAppearance"
        >
          <div
            class="theme-preview__tabs"
            role="group"
            :aria-label="t('appTheme.preview')"
          >
            <NvxButton
              :variant="previewTab === 'components' ? 'secondary' : 'ghost'"
              size="sm"
              :aria-pressed="previewTab === 'components'"
              @click="previewTab = 'components'"
            >
              {{ t('appTheme.previewTab') }}
            </NvxButton><NvxButton
              :variant="previewTab === 'list' ? 'secondary' : 'ghost'"
              size="sm"
              :aria-pressed="previewTab === 'list'"
              @click="previewTab = 'list'"
            >
              {{ t('appTheme.previewTabSecondary') }}
            </NvxButton>
          </div>
          <div
            v-if="previewTab === 'components'"
            class="theme-preview__components"
          >
            <div class="theme-preview__buttons">
              <NvxButton>{{ t('appTheme.primary') }}</NvxButton><NvxButton variant="secondary">
                {{ t('appTheme.secondary') }}
              </NvxButton><NvxButton disabled>
                {{ t('appTheme.disabledButton') }}
              </NvxButton>
            </div>
            <NvxField :label="t('appTheme.input')">
              <NvxInput
                v-model="previewInput"
                :placeholder="t('appTheme.inputPlaceholder')"
                :aria-label="t('appTheme.input')"
              />
            </NvxField>
            <NvxInlineNotice tone="info">
              {{ t('appTheme.previewNotice') }}
            </NvxInlineNotice>
            <NvxInlineNotice tone="error">
              {{ t('appTheme.errorState') }}
            </NvxInlineNotice>
            <NvxButton variant="danger">
              {{ t('appTheme.danger') }}
            </NvxButton>
          </div>
          <table
            v-else
            class="theme-preview__table"
          >
            <thead><tr><th>{{ t('appTheme.name') }}</th><th>{{ t('appTheme.status') }}</th></tr></thead><tbody>
              <tr>
                <td>{{ t('appTheme.sampleHost') }}</td><td>
                  <NvxStatusLabel tone="success">
                    {{ t('appTheme.ready') }}
                  </NvxStatusLabel>
                </td>
              </tr><tr>
                <td>{{ t('appTheme.sampleTask') }}</td><td>
                  <NvxStatusLabel tone="warning">
                    {{ t('appTheme.paused') }}
                  </NvxStatusLabel>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <p class="theme-settings__hint">
          {{ t('appTheme.secureNote') }}
        </p>
      </aside>
    </div>
    <NvxInlineNotice
      v-if="error || !valid"
      tone="error"
      role="alert"
    >
      {{ error.startsWith('preference:') ? t(`applicationPreferenceErrors.${error.slice(11)}`) : t(`appTheme.errors.${error || 'invalid'}`) }}
    </NvxInlineNotice>
    <p
      v-if="message"
      class="theme-settings__feedback"
      role="status"
    >
      {{ t(`appTheme.${message}`) }}
    </p>
    <footer class="theme-settings__actions">
      <div>
        <NvxButton
          variant="secondary"
          size="sm"
          :disabled="busy"
          @click="fileInput?.click()"
        >
          <NvxIcon
            :icon="Upload"
            :size="16"
          />{{ t('appTheme.import') }}
        </NvxButton><NvxButton
          variant="ghost"
          size="sm"
          :disabled="busy"
          @click="exportFile"
        >
          <NvxIcon
            :icon="Download"
            :size="16"
          />{{ t('appTheme.export') }}
        </NvxButton><input
          ref="fileInput"
          type="file"
          accept="application/json,.json"
          hidden
          @change="readFile"
        >
      </div>
      <div>
        <span
          v-if="dirty"
          class="theme-settings__hint"
        >{{ t('appTheme.unsaved') }}</span><NvxButton
          variant="secondary"
          :disabled="!dirty || busy"
          @click="discard"
        >
          {{ t('appTheme.cancel') }}
        </NvxButton><NvxButton
          :disabled="!dirty || !valid || busy || store.loading"
          @click="save"
        >
          {{ t('appTheme.save') }}
        </NvxButton>
      </div>
    </footer>
  </section>
</template>

<style scoped>
.theme-settings { display: grid; gap: var(--nvx-space-5); min-width: 0; }
.theme-settings h2, .theme-settings h3, .theme-settings p { margin: 0; }
.theme-settings h2 { font-size: var(--nvx-font-size-lg); }
.theme-settings h3 { font-size: var(--nvx-font-size-md); }
.theme-settings__heading, .theme-settings__section-heading, .theme-settings__mode { display: flex; justify-content: space-between; align-items: center; gap: var(--nvx-space-4); }
.theme-settings__heading { padding-bottom: var(--nvx-space-5); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.theme-settings__heading p, .theme-settings__mode p, .theme-settings__hint, .theme-settings__colors p { margin-top: var(--nvx-space-2); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.theme-settings__mode > :last-child { width: 200px; flex-shrink: 0; }
.theme-settings__workspace { display: grid; grid-template-columns: minmax(300px, 1fr) minmax(260px, .9fr); gap: var(--nvx-space-6); align-items: start; }
.theme-settings__editor { min-width: 0; display: grid; gap: var(--nvx-space-4); }
.theme-settings__choices { display: grid; grid-template-columns: repeat(auto-fit, minmax(125px, 1fr)); gap: var(--nvx-space-3); }
.theme-choice { display: grid; gap: var(--nvx-space-2); padding: var(--nvx-space-3); text-align: left; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); color: var(--nvx-color-text-primary); cursor: pointer; min-width: 0; }
.theme-choice:hover { background: var(--nvx-color-bg-hover); }
.theme-choice--selected { border-color: var(--nvx-color-accent); box-shadow: 0 0 0 1px var(--nvx-color-accent); }
.theme-choice:disabled { opacity: .55; cursor: not-allowed; }
.theme-choice__swatches { display: flex; height: 36px; overflow: hidden; border-radius: var(--nvx-radius-sm); border: 1px solid var(--nvx-color-border); }
.theme-choice__swatches > span { flex: 1; }
.theme-choice__name { display: flex; justify-content: space-between; align-items: center; gap: var(--nvx-space-2); font-weight: var(--nvx-font-weight-semibold); overflow-wrap: anywhere; }
.theme-choice small { color: var(--nvx-color-text-secondary); }
.theme-settings__fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--nvx-space-4); }
.theme-settings__colors { border-top: var(--nvx-border-width) solid var(--nvx-color-border); padding-top: var(--nvx-space-4); }
.theme-settings__colors summary { display: flex; align-items: center; gap: var(--nvx-space-2); cursor: pointer; font-weight: var(--nvx-font-weight-semibold); }
.theme-settings__colors fieldset { margin: var(--nvx-space-4) 0 0; padding: 0; border: 0; display: grid; gap: var(--nvx-space-2); }
.theme-settings__colors legend { padding: 0 0 var(--nvx-space-3); font-weight: var(--nvx-font-weight-semibold); }
.theme-color { display: grid; grid-template-columns: minmax(100px, 1fr) 32px 100px; gap: var(--nvx-space-2); align-items: center; }
.theme-color input[type=color] { height: 32px; width: 32px; padding: 2px; border: 1px solid var(--nvx-color-border); border-radius: var(--nvx-radius-sm); background: var(--nvx-color-bg-surface); cursor: pointer; }
.theme-settings__preview-area { display: grid; gap: var(--nvx-space-3); position: sticky; top: 0; padding-left: var(--nvx-space-6); border-left: var(--nvx-border-width) solid var(--nvx-color-border); }
.theme-preview { padding: var(--nvx-space-4); display: grid; gap: var(--nvx-space-4); min-height: 320px; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-canvas); color: var(--nvx-color-text-primary); font-family: var(--nvx-font-sans); font-size: var(--nvx-font-size-body); line-height: var(--nvx-line-height-body); box-shadow: var(--nvx-shadow-toast); }
.theme-preview__tabs, .theme-preview__buttons { display: flex; flex-wrap: wrap; align-items: center; gap: var(--nvx-space-2); }
.theme-preview__tabs { border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); padding-bottom: var(--nvx-space-3); }
.theme-preview__components { display: grid; gap: var(--nvx-space-4); align-content: start; }
.theme-preview__components > :last-child { justify-self: start; }
.theme-preview__table { width: 100%; border-collapse: collapse; align-self: start; text-align: left; }
.theme-preview__table th, .theme-preview__table td { padding: var(--nvx-space-3) var(--nvx-space-2); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.theme-preview__table tbody tr:first-child { background: var(--nvx-color-accent-soft); }
.theme-settings__actions, .theme-settings__actions > div { display: flex; flex-wrap: wrap; align-items: center; gap: var(--nvx-space-2); }
.theme-settings__actions { justify-content: space-between; padding-top: var(--nvx-space-4); border-top: var(--nvx-border-width) solid var(--nvx-color-border); }
.theme-settings__feedback { color: var(--nvx-color-success); }
@media (max-width: 1250px) { .theme-settings__workspace { grid-template-columns: minmax(0, 1fr); } .theme-settings__preview-area { position: static; border-left: 0; padding-left: 0; border-top: 1px solid var(--nvx-color-border); padding-top: var(--nvx-space-4); } }
</style>
