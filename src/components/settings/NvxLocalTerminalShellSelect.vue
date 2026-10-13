<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import { getLocalTerminalShellCatalog, type LocalTerminalShellCatalog } from "../../core-api/local-terminal-shells";
import {
  DEFAULT_LOCAL_TERMINAL_SHELL,
  customLocalTerminalShell,
  localTerminalShellPath,
  localTerminalShellPathIssue,
  type LocalTerminalShellPathIssue,
} from "../../ui-transfer";
import { NvxInlineNotice, NvxInput, NvxSelect } from "../ui";

const props = defineProps<{ modelValue: string; ariaLabel?: string }>();
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const { t } = useI18n();

// UI-only sentinel for "custom path"; it is never written to the preference.
const CUSTOM = "__custom__";

const catalog = ref<LocalTerminalShellCatalog>({ defaultShellName: "", options: [] });
const catalogLoaded = ref(false);
const customMode = ref(false);
const customText = ref(localTerminalShellPath(props.modelValue) ?? "");
const customIssue = ref<LocalTerminalShellPathIssue | null>(null);

onMounted(async () => {
  catalog.value = await getLocalTerminalShellCatalog();
  catalogLoaded.value = true;
});

watch(() => props.modelValue, (value) => {
  const path = localTerminalShellPath(value);
  if (path !== null) customText.value = path;
});

const inCatalog = computed(() => catalog.value.options.some((option) => option.id === props.modelValue));
const customPathSaved = computed(() => localTerminalShellPath(props.modelValue) !== null && !inCatalog.value);
// A fixed id (for example "cmd" read on macOS) that this machine does not offer.
const unavailableFixedId = computed(() => catalogLoaded.value && props.modelValue !== DEFAULT_LOCAL_TERMINAL_SHELL
  && !inCatalog.value && localTerminalShellPath(props.modelValue) === null);

const selectValue = computed(() => customMode.value || customPathSaved.value ? CUSTOM : props.modelValue);

const selectOptions = computed(() => {
  const names = catalog.value.options.map((option) => option.name);
  const defaultName = catalog.value.defaultShellName;
  const options = [
    {
      value: DEFAULT_LOCAL_TERMINAL_SHELL,
      label: defaultName
        ? t("sshSettings.applicationPreferences.localTerminalShellDefault", { name: defaultName })
        : t("sshSettings.applicationPreferences.localTerminalShellDefaultUnknown"),
    },
    ...catalog.value.options.map((option) => ({
      value: option.id,
      // Same-named shells in different directories are told apart by their path.
      label: names.filter((name) => name === option.name).length > 1 && option.path ? `${option.name} (${option.path})` : option.name,
    })),
  ];
  if (unavailableFixedId.value) {
    options.push({
      value: props.modelValue,
      label: t("sshSettings.applicationPreferences.localTerminalShellUnavailableOption", { id: props.modelValue }),
    });
  }
  options.push({ value: CUSTOM, label: t("sshSettings.applicationPreferences.localTerminalShellCustom") });
  return options;
});

function select(value: string) {
  if (value === CUSTOM) {
    customMode.value = true;
    customIssue.value = null;
    return;
  }
  customMode.value = false;
  customIssue.value = null;
  if (value !== props.modelValue) emit("update:modelValue", value);
}

function commitCustom() {
  const issue = localTerminalShellPathIssue(customText.value);
  customIssue.value = issue;
  if (issue) return;
  const shell = customLocalTerminalShell(customText.value);
  if (shell !== props.modelValue) emit("update:modelValue", shell);
}
</script>

<template>
  <div class="local-terminal-shell">
    <NvxSelect
      :model-value="selectValue"
      :options="selectOptions"
      :aria-label="ariaLabel"
      @update:model-value="select"
    />
    <template v-if="selectValue === CUSTOM">
      <NvxInput
        v-model="customText"
        :aria-label="t('sshSettings.applicationPreferences.localTerminalShellPath')"
        :placeholder="t('sshSettings.applicationPreferences.localTerminalShellPathPlaceholder')"
        :invalid="customIssue !== null"
        @update:model-value="customIssue = null"
        @keydown.enter.prevent="commitCustom"
        @blur="commitCustom"
      />
      <small
        v-if="customIssue"
        class="local-terminal-shell__issue"
        role="alert"
      >
        {{ t(`sshSettings.applicationPreferences.localTerminalShellPathErrors.${customIssue}`) }}
      </small>
    </template>
    <NvxInlineNotice
      v-if="unavailableFixedId"
      tone="warning"
    >
      {{ t("sshSettings.applicationPreferences.localTerminalShellUnavailable") }}
    </NvxInlineNotice>
  </div>
</template>

<style scoped>
.local-terminal-shell {
  display: grid;
  gap: var(--nvx-space-2);
  min-width: 0;
}

.local-terminal-shell__issue {
  color: var(--nvx-color-danger);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
}
</style>
