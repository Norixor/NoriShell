<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import { listHostCatalog } from "../../core-api/client";
import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";
import { validateInteractionPreferences, type DoubleClickSelection, type InteractionPreferences, type RightClickBehavior, type TerminalBellMode, type TerminalBackspaceMode } from "../../terminal/interaction-preferences";
import { validateTerminalKeyboardPreferences, type HostKeyboardMode, type TerminalKeyboardPreferences } from "../../terminal/keyboard-compatibility";
import { useTipsStore } from "../../stores/tips";
import { NvxButton, NvxField, NvxInlineNotice, NvxInput, NvxSelect } from "../ui";

interface InteractionDraft {
  scrollback: string;
  scrollSensitivity: string;
  smoothScrollDuration: InteractionPreferences["smoothScrollDuration"];
  doubleClickSelection: DoubleClickSelection;
  copyOnSelect: boolean;
  rightClickBehavior: RightClickBehavior;
  optionAsMetaLeft: boolean;
  optionAsMetaRight: boolean;
  backspaceMode: TerminalBackspaceMode;
  bellMode: TerminalBellMode;
  linksEnabled: boolean;
}

interface HostOption {
  hostId: string;
  label: string;
}

const { t } = useI18n();
const preferences = useTerminalPreferencesStore();
const tips = useTipsStore();
function createDraft(interaction: InteractionPreferences): InteractionDraft {
  return {
    scrollback: String(interaction.scrollback),
    scrollSensitivity: String(interaction.scrollSensitivity),
    smoothScrollDuration: interaction.smoothScrollDuration,
    doubleClickSelection: interaction.doubleClickSelection,
    copyOnSelect: interaction.copyOnSelect,
    rightClickBehavior: interaction.rightClickBehavior,
    optionAsMetaLeft: interaction.optionAsMetaLeft,
    optionAsMetaRight: interaction.optionAsMetaRight,
    backspaceMode: interaction.backspaceMode,
    bellMode: interaction.bellMode,
    linksEnabled: interaction.linksEnabled,
  };
}
const draft = ref<InteractionDraft>(createDraft(preferences.preferences.interaction));
const hostOptions = ref<HostOption[]>([]);
const hostCatalogLoading = ref(true);
const hostCatalogFailed = ref(false);
const keyboardScope = ref("");
const hostKeyboardMode = ref<HostKeyboardMode>("inherit");
const hostKeyboardDraft = ref<TerminalKeyboardPreferences>(preferences.resolvedKeyboard());
const hostKeyboardBaseline = ref("");

const smoothOptions = computed(() => ([0, 100, 200] as const).map((value) => ({
  value: String(value),
  label: value === 0 ? t("terminalInteraction.smoothOff") : t("terminalInteraction.smoothMilliseconds", { duration: value }),
})));
const selectionOptions = computed(() => (["word", "path", "address"] as const).map((value) => ({ value, label: t(`terminalInteraction.${value}`) })));
const rightClickOptions = computed(() => (["menu", "paste"] as const).map((value) => ({ value, label: t(`terminalInteraction.rightClick${value === "menu" ? "Menu" : "Paste"}`) })));
const optionMetaOptions = computed(() => ([
  { value: "false", label: t("terminalInteraction.optionMetaOff") },
  { value: "true", label: t("terminalInteraction.optionMetaOn") },
]));
const backspaceOptions = computed(() => (["del", "bs"] as const).map((value) => ({ value, label: t(`terminalInteraction.backspace${value === "del" ? "Del" : "Bs"}`) })));
const bellOptions = computed(() => (["off", "visual", "sound"] as const).map((value) => ({ value, label: t(`terminalInteraction.bell${value[0]!.toUpperCase()}${value.slice(1)}`) })));
const linkOptions = computed(() => ([
  { value: "true", label: t("terminalInteraction.linksOn") },
  { value: "false", label: t("terminalInteraction.linksOff") },
]));
const hostScopeOptions = computed(() => [
  { value: "", label: t("terminalInteraction.hostScopeGlobal") },
  ...hostOptions.value.map((host) => ({ value: host.hostId, label: host.label })),
]);
const hostKeyboardModeOptions = computed(() => (["inherit", "override"] as const).map((value) => ({ value, label: t(`terminalInteraction.hostKeyboard${value[0]!.toUpperCase()}${value.slice(1)}`) })));
const interaction = computed<InteractionPreferences>(() => ({
  scrollback: Number(draft.value.scrollback),
  scrollSensitivity: Number(draft.value.scrollSensitivity),
  smoothScrollDuration: draft.value.smoothScrollDuration,
  doubleClickSelection: draft.value.doubleClickSelection,
  copyOnSelect: draft.value.copyOnSelect,
  rightClickBehavior: draft.value.rightClickBehavior,
  optionAsMetaLeft: draft.value.optionAsMetaLeft,
  optionAsMetaRight: draft.value.optionAsMetaRight,
  backspaceMode: draft.value.backspaceMode,
  bellMode: draft.value.bellMode,
  linksEnabled: draft.value.linksEnabled,
}));
const valid = computed(() => validateInteractionPreferences(interaction.value));
const scrollbackInvalid = computed(() => !Number.isInteger(interaction.value.scrollback) || interaction.value.scrollback < 1_000 || interaction.value.scrollback > 100_000);
const scrollSensitivityInvalid = computed(() => !Number.isInteger(interaction.value.scrollSensitivity) || interaction.value.scrollSensitivity < 1 || interaction.value.scrollSensitivity > 10);
const dirty = computed(() => JSON.stringify(interaction.value) !== JSON.stringify(preferences.preferences.interaction));
const hostKeyboardValid = computed(() => validateTerminalKeyboardPreferences(hostKeyboardDraft.value));
const hostKeyboardDirty = computed(() => keyboardScope.value !== "" && JSON.stringify({ mode: hostKeyboardMode.value, keyboard: hostKeyboardDraft.value }) !== hostKeyboardBaseline.value);

function report(saved: boolean) {
  tips.show({ scope: "terminal-interaction-settings", tone: saved ? "success" : "error", title: t(`terminalInteraction.${saved ? "saved" : "saveFailed"}`) });
}
function save() {
  if (!valid.value) return;
  report(preferences.setInteraction({ ...interaction.value }));
}
function restore() { draft.value = createDraft(preferences.preferences.interaction); }
function numberValue(key: "scrollback" | "scrollSensitivity", value: string) {
  draft.value = { ...draft.value, [key]: value };
}
function selectDuration(value: string) {
  draft.value = { ...draft.value, smoothScrollDuration: Number(value) as InteractionPreferences["smoothScrollDuration"] };
}
function selectMode(value: string) {
  draft.value = { ...draft.value, doubleClickSelection: value as DoubleClickSelection };
}
function selectRightClickBehavior(value: string) {
  draft.value = { ...draft.value, rightClickBehavior: value as RightClickBehavior };
}
function selectCopyOnSelect(value: string) {
  draft.value = { ...draft.value, copyOnSelect: value === "true" };
}
function selectOptionMeta(side: "optionAsMetaLeft" | "optionAsMetaRight", value: string) {
  draft.value = { ...draft.value, [side]: value === "true" };
}
function selectBackspace(value: string) {
  draft.value = { ...draft.value, backspaceMode: value as TerminalBackspaceMode };
}
function selectBell(value: string) {
  draft.value = { ...draft.value, bellMode: value as TerminalBellMode };
}
function selectLinks(value: string) {
  draft.value = { ...draft.value, linksEnabled: value === "true" };
}
function resetHostKeyboard() {
  if (!keyboardScope.value) {
    hostKeyboardMode.value = "inherit";
    hostKeyboardDraft.value = preferences.resolvedKeyboard();
    hostKeyboardBaseline.value = "";
    return;
  }
  const stored = preferences.preferences.hostKeyboard[keyboardScope.value];
  hostKeyboardMode.value = stored?.mode ?? "inherit";
  hostKeyboardDraft.value = preferences.resolvedKeyboard(keyboardScope.value);
  hostKeyboardBaseline.value = JSON.stringify({ mode: hostKeyboardMode.value, keyboard: hostKeyboardDraft.value });
}
function selectHostKeyboardMode(value: string) {
  hostKeyboardMode.value = value as HostKeyboardMode;
  if (hostKeyboardMode.value === "override" && !preferences.preferences.hostKeyboard[keyboardScope.value]) {
    hostKeyboardDraft.value = preferences.resolvedKeyboard();
  }
}
function selectHostKeyboardOption(key: keyof TerminalKeyboardPreferences, value: string) {
  hostKeyboardDraft.value = {
    ...hostKeyboardDraft.value,
    [key]: key === "backspaceMode" ? value as TerminalBackspaceMode : value === "true",
  } as TerminalKeyboardPreferences;
}
function saveHostKeyboard() {
  if (!keyboardScope.value || !hostKeyboardValid.value) return;
  const saved = preferences.setHostKeyboard(
    keyboardScope.value,
    hostKeyboardMode.value === "inherit"
      ? { mode: "inherit" }
      : { mode: "override", keyboard: { ...hostKeyboardDraft.value } },
  );
  tips.show({ scope: "terminal-host-keyboard-settings", tone: saved ? "success" : "error", title: t(`terminalInteraction.${saved ? "hostKeyboardSaved" : "hostKeyboardSaveFailed"}`) });
  if (saved) resetHostKeyboard();
}

watch(keyboardScope, resetHostKeyboard, { immediate: true });
onMounted(async () => {
  try {
    // Keep only the HostId and display label; never give full Host or credential objects to local preference state.
    hostOptions.value = (await listHostCatalog()).map(({ host }) => ({
      hostId: host.hostId,
      label: host.label || host.address,
    }));
  } catch {
    hostCatalogFailed.value = true;
  } finally {
    hostCatalogLoading.value = false;
  }
});
</script>

<template>
  <section
    class="nvx-terminal-interaction-settings"
    :aria-label="t('terminalInteraction.title')"
  >
    <header>
      <h2>{{ t('terminalInteraction.title') }}</h2>
      <p>{{ t('terminalInteraction.description') }}</p>
    </header>
    <div class="nvx-terminal-interaction-settings__fields">
      <NvxField
        :label="t('terminalInteraction.scrollback')"
        :hint="t('terminalInteraction.scrollbackHint')"
        :error="scrollbackInvalid ? t('terminalInteraction.scrollbackInvalid') : undefined"
      >
        <NvxInput
          :model-value="draft.scrollback"
          type="number"
          :min="1000"
          :max="100000"
          :step="1000"
          :aria-label="t('terminalInteraction.scrollback')"
          :invalid="scrollbackInvalid"
          @update:model-value="numberValue('scrollback', $event)"
        />
      </NvxField>
      <NvxField
        :label="t('terminalInteraction.scrollSensitivity')"
        :hint="t('terminalInteraction.scrollSensitivityHint')"
        :error="scrollSensitivityInvalid ? t('terminalInteraction.scrollSensitivityInvalid') : undefined"
      >
        <NvxInput
          :model-value="draft.scrollSensitivity"
          type="number"
          :min="1"
          :max="10"
          :step="1"
          :aria-label="t('terminalInteraction.scrollSensitivity')"
          :invalid="scrollSensitivityInvalid"
          @update:model-value="numberValue('scrollSensitivity', $event)"
        />
      </NvxField>
      <NvxField
        :label="t('terminalInteraction.smoothScrollDuration')"
        :hint="t('terminalInteraction.smoothScrollDurationHint')"
      >
        <NvxSelect
          :model-value="String(draft.smoothScrollDuration)"
          :options="smoothOptions"
          :aria-label="t('terminalInteraction.smoothScrollDuration')"
          @update:model-value="selectDuration"
        />
      </NvxField>
      <NvxField
        :label="t('terminalInteraction.doubleClickSelection')"
        :hint="t('terminalInteraction.doubleClickSelectionHint')"
      >
        <NvxSelect
          :model-value="draft.doubleClickSelection"
          :options="selectionOptions"
          :aria-label="t('terminalInteraction.doubleClickSelection')"
          @update:model-value="selectMode"
        />
      </NvxField>
      <NvxField
        :label="t('terminalInteraction.copyOnSelect')"
        :hint="t('terminalInteraction.copyOnSelectHint')"
      >
        <NvxSelect
          :model-value="String(draft.copyOnSelect)"
          :options="[{ value: 'false', label: t('terminalInteraction.copyOnSelectOff') }, { value: 'true', label: t('terminalInteraction.copyOnSelectOn') }]"
          :aria-label="t('terminalInteraction.copyOnSelect')"
          @update:model-value="selectCopyOnSelect"
        />
      </NvxField>
      <NvxField
        :label="t('terminalInteraction.rightClickBehavior')"
        :hint="t('terminalInteraction.rightClickBehaviorHint')"
      >
        <NvxSelect
          :model-value="draft.rightClickBehavior"
          :options="rightClickOptions"
          :aria-label="t('terminalInteraction.rightClickBehavior')"
          @update:model-value="selectRightClickBehavior"
        />
      </NvxField>
      <NvxField
        :label="t('terminalInteraction.optionAsMetaLeft')"
        :hint="t('terminalInteraction.optionAsMetaHint')"
      >
        <NvxSelect
          :model-value="String(draft.optionAsMetaLeft)"
          :options="optionMetaOptions"
          :aria-label="t('terminalInteraction.optionAsMetaLeft')"
          @update:model-value="selectOptionMeta('optionAsMetaLeft', $event)"
        />
      </NvxField>
      <NvxField :label="t('terminalInteraction.optionAsMetaRight')">
        <NvxSelect
          :model-value="String(draft.optionAsMetaRight)"
          :options="optionMetaOptions"
          :aria-label="t('terminalInteraction.optionAsMetaRight')"
          @update:model-value="selectOptionMeta('optionAsMetaRight', $event)"
        />
      </NvxField>
      <NvxField
        :label="t('terminalInteraction.backspaceMode')"
        :hint="t('terminalInteraction.backspaceModeHint')"
      >
        <NvxSelect
          :model-value="draft.backspaceMode"
          :options="backspaceOptions"
          :aria-label="t('terminalInteraction.backspaceMode')"
          @update:model-value="selectBackspace"
        />
      </NvxField>
      <NvxField
        :label="t('terminalInteraction.bellMode')"
        :hint="t('terminalInteraction.bellModeHint')"
      >
        <NvxSelect
          :model-value="draft.bellMode"
          :options="bellOptions"
          :aria-label="t('terminalInteraction.bellMode')"
          @update:model-value="selectBell"
        />
      </NvxField>
      <NvxField
        :label="t('terminalInteraction.linksEnabled')"
        :hint="t('terminalInteraction.linksEnabledHint')"
      >
        <NvxSelect
          :model-value="String(draft.linksEnabled)"
          :options="linkOptions"
          :aria-label="t('terminalInteraction.linksEnabled')"
          @update:model-value="selectLinks"
        />
      </NvxField>
    </div>
    <div class="nvx-terminal-interaction-settings__actions">
      <NvxButton
        variant="secondary"
        :disabled="!dirty"
        @click="restore"
      >
        {{ t('terminalInteraction.restore') }}
      </NvxButton>
      <NvxButton
        :disabled="!dirty || !valid"
        @click="save"
      >
        {{ t('terminalInteraction.save') }}
      </NvxButton>
    </div>
    <section class="nvx-terminal-interaction-settings__host-input">
      <header>
        <h3>{{ t('terminalInteraction.hostKeyboardTitle') }}</h3>
        <p>{{ t('terminalInteraction.hostKeyboardDescription') }}</p>
      </header>
      <NvxInlineNotice
        v-if="hostCatalogFailed"
        tone="warning"
        :title="t('terminalInteraction.hostCatalogFailed')"
      />
      <p
        v-else-if="hostCatalogLoading"
        class="nvx-terminal-interaction-settings__hint"
      >
        {{ t('terminalInteraction.hostCatalogLoading') }}
      </p>
      <div class="nvx-terminal-interaction-settings__fields">
        <NvxField :label="t('terminalInteraction.hostScope')">
          <NvxSelect
            v-model="keyboardScope"
            :options="hostScopeOptions"
            :aria-label="t('terminalInteraction.hostScope')"
          />
        </NvxField>
        <NvxField
          v-if="keyboardScope"
          :label="t('terminalInteraction.hostKeyboardMode')"
        >
          <NvxSelect
            :model-value="hostKeyboardMode"
            :options="hostKeyboardModeOptions"
            :aria-label="t('terminalInteraction.hostKeyboardMode')"
            @update:model-value="selectHostKeyboardMode"
          />
        </NvxField>
        <template v-if="keyboardScope && hostKeyboardMode === 'override'">
          <NvxField :label="t('terminalInteraction.optionAsMetaLeft')">
            <NvxSelect
              :model-value="String(hostKeyboardDraft.optionAsMetaLeft)"
              :options="optionMetaOptions"
              :aria-label="t('terminalInteraction.optionAsMetaLeft')"
              @update:model-value="selectHostKeyboardOption('optionAsMetaLeft', $event)"
            />
          </NvxField>
          <NvxField :label="t('terminalInteraction.optionAsMetaRight')">
            <NvxSelect
              :model-value="String(hostKeyboardDraft.optionAsMetaRight)"
              :options="optionMetaOptions"
              :aria-label="t('terminalInteraction.optionAsMetaRight')"
              @update:model-value="selectHostKeyboardOption('optionAsMetaRight', $event)"
            />
          </NvxField>
          <NvxField :label="t('terminalInteraction.backspaceMode')">
            <NvxSelect
              :model-value="hostKeyboardDraft.backspaceMode"
              :options="backspaceOptions"
              :aria-label="t('terminalInteraction.backspaceMode')"
              @update:model-value="selectHostKeyboardOption('backspaceMode', $event)"
            />
          </NvxField>
        </template>
      </div>
      <p
        v-if="keyboardScope && hostKeyboardMode === 'inherit'"
        class="nvx-terminal-interaction-settings__hint"
      >
        {{ t('terminalInteraction.hostKeyboardInheritHint') }}
      </p>
      <div
        v-if="keyboardScope"
        class="nvx-terminal-interaction-settings__actions"
      >
        <NvxButton
          variant="secondary"
          :disabled="!hostKeyboardDirty"
          @click="resetHostKeyboard"
        >
          {{ t('terminalInteraction.restore') }}
        </NvxButton>
        <NvxButton
          :disabled="!hostKeyboardDirty || !hostKeyboardValid"
          @click="saveHostKeyboard"
        >
          {{ t('terminalInteraction.save') }}
        </NvxButton>
      </div>
    </section>
  </section>
</template>

<style scoped>
.nvx-terminal-interaction-settings { display: grid; gap: var(--nvx-space-5); min-width: 0; }
.nvx-terminal-interaction-settings h2 { margin: 0; font-size: var(--nvx-font-size-lg); }
.nvx-terminal-interaction-settings p { margin: 5px 0 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.nvx-terminal-interaction-settings__fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--nvx-space-4); }
.nvx-terminal-interaction-settings__actions { display: flex; justify-content: flex-end; gap: var(--nvx-space-2); }
.nvx-terminal-interaction-settings__host-input { display: grid; gap: var(--nvx-space-4); padding-top: var(--nvx-space-4); border-top: var(--nvx-border-width) solid var(--nvx-color-border); }
.nvx-terminal-interaction-settings__host-input h3 { margin: 0; font-size: var(--nvx-font-size-md); }
.nvx-terminal-interaction-settings__hint { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
@media (max-width: 760px) { .nvx-terminal-interaction-settings__fields { grid-template-columns: minmax(0, 1fr); } }
</style>
