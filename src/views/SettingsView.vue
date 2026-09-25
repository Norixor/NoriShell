<script setup lang="ts">
import {
  Check,
  Globe2,
  KeyRound,
  LockKeyhole,
  Palette,
  Pencil,
  Plus,
  Power,
  Settings,
  ShieldCheck,
  SquareX,
  Type,
  SquareTerminal,
  Highlighter,
  Info,
  Keyboard,
  SlidersHorizontal,
} from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute } from "vue-router";
import NvxAppThemeSettings from "../components/settings/NvxAppThemeSettings.vue";
import NvxTerminalInteractionSettings from "../components/settings/NvxTerminalInteractionSettings.vue";
import NvxPreferenceTransferSettings from "../components/settings/NvxPreferenceTransferSettings.vue";
import NvxDesktopPreferencesSettings from "../components/settings/NvxDesktopPreferencesSettings.vue";
import NvxSftpSettings from "../components/settings/NvxSftpSettings.vue";
import NvxHighlightSettings from "../components/settings/NvxHighlightSettings.vue";
import NvxShortcutSettings from "../components/settings/NvxShortcutSettings.vue";
import NvxTerminalEnhancementSettings from "../components/settings/NvxTerminalEnhancementSettings.vue";
import NvxNativeShellSettings from "../components/settings/NvxNativeShellSettings.vue";
import NvxNativeNotificationSettings from "../components/settings/NvxNativeNotificationSettings.vue";
import NvxReleaseSettings from "../components/settings/NvxReleaseSettings.vue";
import IdentitiesSettingsView from "./IdentitiesSettingsView.vue";
import KnownHostsSettingsView from "./KnownHostsSettingsView.vue";
import OfflineBackup from "./OfflineBackup.vue";
import { useNativeTerminalStore } from "../stores/nativeTerminal";
import { useAppUpdateStore } from "../stores/appUpdate";

import { NvxPluginExtensionTarget } from "../components/plugins";
import {
  NvxButton,
  NvxCheckbox,
  NvxDialog,
  NvxField,
  NvxIcon,
  NvxInlineNotice,
  NvxInput,
  NvxSelect,
  NvxStatusLabel,
} from "../components/ui";
import {
  disableVaultAutoUnlock,
  fetchVaultStatus,
  lockVault,
  parseCoreApiError,
  setPluginLocale,
  listHostCatalog,
} from "../core-api/client";
import { requestSecureVault, type SecureVaultMode } from "../core-api/secure-vault-client";
import type { VaultStatus } from "../core-api/generated/core-api";
import { resolveLocale, type LocalePreference } from "../locales";
import { useTipsStore } from "../stores/tips";
import {
  useUiStore,
  type NewTerminalBehavior,
  type SinglePaneTabCloseBehavior,
  type TerminalStartupBehavior,
} from "../stores/ui";
import { UI_ZOOM_LEVELS } from "../ui-zoom";
import { detectInstalledTerminalFonts } from "../terminal-fonts";
import {
  cloneTerminalPalette,
  MAX_TERMINAL_FONT_SIZE,
  MAX_TERMINAL_LETTER_SPACING,
  MAX_TERMINAL_LINE_HEIGHT,
  MIN_TERMINAL_FONT_SIZE,
  MIN_TERMINAL_LETTER_SPACING,
  MIN_TERMINAL_LINE_HEIGHT,
  isHexColor,
  isTerminalPresetId,
  parseTerminalPalette,
  terminalPreset,
  TERMINAL_PRESETS,
  TERMINAL_FONT_WEIGHTS,
  type TerminalCursorStyle,
  type TerminalColorKey,
  type TerminalPalette,
  type TerminalPresetId,
  type TerminalThemeMode,
} from "../terminal-theme";

const { t, te } = useI18n();
const route = useRoute();
const ui = useUiStore();
const tips = useTipsStore();
const nativeTerminal = useNativeTerminalStore();
const appUpdate = useAppUpdateStore();

const securityLinks = [
  { key: "hostKeys", icon: ShieldCheck, section: "knownHosts" },
  { key: "identities", icon: KeyRound, section: "identities" },
] as const;

type SettingsSection = "appearance" | "transfer" | "desktop" | "interaction" | "files" | "application" | "terminal" | "vault" | "knownHosts" | "identities" | "enhancements" | "highlights" | "shortcuts" | "about";

const activeSection = ref<SettingsSection>("application");
const enhancementSections = [
  { id: "appearance" as const, icon: Palette, label: "appTheme.title" },
  { id: "desktop" as const, icon: Settings, label: "desktopPreferences.title" },
  { id: "interaction" as const, icon: SlidersHorizontal, label: "terminalInteraction.title" },
  { id: "files" as const, icon: SlidersHorizontal, label: "sftpSettings.title" },
  { id: "enhancements" as const, icon: SlidersHorizontal, label: "terminalEnhancements.title" },
  { id: "highlights" as const, icon: Highlighter, label: "terminalEnhancements.highlights" },
  { id: "shortcuts" as const, icon: Keyboard, label: "terminalEnhancements.shortcuts" },
  { id: "transfer" as const, icon: Settings, label: "preferenceTransfer.title" },
];
const highlightHosts = ref<{ hostId: string; label: string }[]>([]);
const highlightHostsFailed = ref(false);
async function loadHighlightHosts() {
  highlightHostsFailed.value = false;
  try { highlightHosts.value = (await listHostCatalog()).map(({ host }) => ({ hostId: host.hostId, label: host.label || host.address })); }
  catch { highlightHostsFailed.value = true; }
}
watch(activeSection, (section) => { if (section === "highlights") void loadHighlightHosts(); });
watch(() => route.query.section, (section) => {
  if (typeof section === "string" && ["appearance", "application", "terminal", "vault", "knownHosts", "identities", "enhancements", "highlights", "shortcuts", "files", "interaction", "desktop", "transfer", "about"].includes(section)) activeSection.value = section as SettingsSection;
}, { immediate: true });

const uiZoomOptions = UI_ZOOM_LEVELS.map((value) => ({ value: String(value), label: `${value}%` }));

async function setUiZoom(value: string) {
  if (await ui.setUiZoom(Number(value))) return;
  tips.show({ scope: "settings-zoom", tone: "error", title: t("sshSettings.applicationPreferences.zoomFailed") });
}

const localeOptions = computed(() => [
  { value: "system", label: t("sshSettings.applicationPreferences.locales.system") },
  { value: "zh-CN", label: t("sshSettings.applicationPreferences.locales.zhCN") },
  { value: "en", label: t("sshSettings.applicationPreferences.locales.en") },
]);

const terminalStartupBehaviorOptions = computed(() => [
  {
    value: "welcome",
    label: t("sshSettings.applicationPreferences.startupBehaviors.welcome"),
  },
  {
    value: "restoreHistory",
    label: t("sshSettings.applicationPreferences.startupBehaviors.restoreHistory"),
  },
]);

const newTerminalBehaviorOptions = computed(() => [
  {
    value: "welcome",
    label: t("sshSettings.applicationPreferences.newTerminalBehaviors.welcome"),
  },
  {
    value: "localTerminal",
    label: t("sshSettings.applicationPreferences.newTerminalBehaviors.localTerminal"),
  },
]);

const singlePaneTabCloseBehaviorOptions = computed(() => [
  {
    value: "confirm",
    label: t("sshSettings.applicationPreferences.singlePaneTabCloseBehaviors.confirm"),
  },
  {
    value: "closeDirectly",
    label: t("sshSettings.applicationPreferences.singlePaneTabCloseBehaviors.closeDirectly"),
  },
]);

const terminalColorGroups: ReadonlyArray<{
  labelKey: string;
  colors: readonly TerminalColorKey[];
}> = [
  {
    labelKey: "sshSettings.terminalAppearance.groups.canvas",
    colors: ["background", "foreground", "muted", "cursor", "selection"],
  },
  {
    labelKey: "sshSettings.terminalAppearance.groups.ansi",
    colors: ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"],
  },
  {
    labelKey: "sshSettings.terminalAppearance.groups.brightAnsi",
    colors: [
      "brightBlack",
      "brightRed",
      "brightGreen",
      "brightYellow",
      "brightBlue",
      "brightMagenta",
      "brightCyan",
      "brightWhite",
    ],
  },
];

const previewAnsiColors: readonly TerminalColorKey[] = [
  "red",
  "yellow",
  "green",
  "cyan",
  "blue",
  "magenta",
];

const customEditorOpen = ref(false);
const customBase = ref<"current" | TerminalPresetId>("current");
const customName = ref("");
const customPalette = ref<TerminalPalette>(cloneTerminalPalette(ui.resolvedTerminalPalette));
const customError = ref<"name" | "colors" | "storage" | "">("");
const terminalFontSizeDraft = ref(String(ui.terminalFontSize));
const terminalLineHeightDraft = ref(String(ui.terminalLineHeight));
const terminalLetterSpacingDraft = ref(String(ui.terminalLetterSpacing));
const installedTerminalFonts = ref<string[]>([]);
const terminalFontsLoading = ref(true);
const vaultStatus = ref<VaultStatus | null>(null);
const vaultStatusLoading = ref(true);
const vaultActionLoading = ref(false);
const vaultActionError = ref("");
const vaultPolicyOptions = computed(() => [
  {
    value: "currentSession",
    label: t("sshSettings.vault.policies.currentSession"),
  },
  {
    value: "automatic",
    label: t("sshSettings.vault.policies.automatic"),
  },
  {
    value: "automaticLocal",
    label: t("sshSettings.vault.policies.automaticLocal"),
  },
]);
const vaultPolicyHint = computed(() => t(
  vaultStatus.value?.unlockPolicy === "automaticLocal"
    ? "sshSettings.vault.localLockHint"
    : "sshSettings.vault.policyHint",
));

const vaultStateLabel = computed(() => {
  if (vaultStatusLoading.value) return t("sshSettings.vault.states.loading");
  if (!vaultStatus.value) return t("sshSettings.vault.states.unavailable");
  return t(`sshSettings.vault.states.${vaultStatus.value.state}`);
});

const vaultStateTone = computed(() => {
  if (vaultStatus.value?.autoUnlockFailure) return "warning" as const;
  if (vaultStatus.value?.state === "unlocked") return "success" as const;
  return "neutral" as const;
});

const autoUnlockFailureMessage = computed(() => {
  const failure = vaultStatus.value?.autoUnlockFailure;
  if (!failure) return "";

  const policy = vaultStatus.value?.unlockPolicy === "automaticLocal"
    ? "automaticLocal"
    : "automatic";
  const key = `sshSettings.vault.autoUnlockFailures.${policy}.${failure}`;
  return te(key) ? t(key) : t(`sshSettings.vault.autoUnlockFailures.automatic.${failure}`);
});

const autoUnlockRepairPolicy = computed(() => (
  vaultStatus.value?.unlockPolicy === "automaticLocal" ? "automaticLocal" : "automatic"
));

const terminalFontOptions = computed(() => [
  {
    value: "",
    label: t("sshSettings.terminalAppearance.typography.systemMonospace"),
  },
  ...installedTerminalFonts.value.map((fontFamily) => ({
    value: fontFamily,
    label: fontFamily,
  })),
]);

const terminalFontWeightOptions = computed(() =>
  TERMINAL_FONT_WEIGHTS.map((weight) => ({
    value: String(weight),
    label: t(`sshSettings.terminalAppearance.typography.weights.${weight}`),
  })),
);

const terminalCursorStyleOptions = computed(() => [
  { value: "block", label: t("sshSettings.terminalAppearance.typography.cursorStyles.block") },
  { value: "underline", label: t("sshSettings.terminalAppearance.typography.cursorStyles.underline") },
  { value: "bar", label: t("sshSettings.terminalAppearance.typography.cursorStyles.bar") },
]);

const terminalFontSizeInvalid = computed(() => {
  const value = Number(terminalFontSizeDraft.value);
  return !Number.isInteger(value)
    || value < MIN_TERMINAL_FONT_SIZE
    || value > MAX_TERMINAL_FONT_SIZE;
});

const terminalLineHeightInvalid = computed(() => {
  const value = Number(terminalLineHeightDraft.value);
  return !Number.isFinite(value)
    || value < MIN_TERMINAL_LINE_HEIGHT
    || value > MAX_TERMINAL_LINE_HEIGHT;
});

const terminalLetterSpacingInvalid = computed(() => {
  const value = Number(terminalLetterSpacingDraft.value);
  return !Number.isInteger(value)
    || value < MIN_TERMINAL_LETTER_SPACING
    || value > MAX_TERMINAL_LETTER_SPACING;
});

const customBaseOptions = computed(() => [
  {
    value: "current",
    label: t("sshSettings.terminalAppearance.customEditor.currentBase"),
  },
  ...TERMINAL_PRESETS.map((preset) => ({
    value: preset.id,
    label: t(preset.nameKey),
  })),
]);

const customDisplayName = computed(() =>
  ui.customTerminalPaletteName
  || t("sshSettings.terminalAppearance.customEditor.defaultName"),
);

const customErrorMessage = computed(() => {
  if (!customError.value) return "";
  return t(`sshSettings.terminalAppearance.customEditor.errors.${customError.value}`);
});

async function setLocale(value: string) {
  const locale = value as LocalePreference;
  try {
    await setPluginLocale(resolveLocale(locale));
    ui.setLocale(locale);
  } catch {
    tips.show({
      tone: "error",
      title: t("sshSettings.applicationPreferences.languageChangeFailed"),
    });
  }
}

function setTerminalStartupBehavior(value: string) {
  ui.setTerminalStartupBehavior(value as TerminalStartupBehavior);
}

function setNewTerminalBehavior(value: string) {
  ui.setNewTerminalBehavior(value as NewTerminalBehavior);
}

function setSinglePaneTabCloseBehavior(value: string) {
  ui.setSinglePaneTabCloseBehavior(value as SinglePaneTabCloseBehavior);
}

function selectTerminalScheme(mode: TerminalThemeMode) {
  ui.setTerminalThemeMode(mode);
}

function setTerminalFontFamily(value: string) {
  ui.setTerminalFontFamily(value);
}

function updateTerminalFontSize(value: string) {
  terminalFontSizeDraft.value = value;
  if (!terminalFontSizeInvalid.value) ui.setTerminalFontSize(value);
}

function restoreTerminalFontSizeDraft() {
  terminalFontSizeDraft.value = String(ui.terminalFontSize);
}

function updateTerminalLineHeight(value: string) {
  terminalLineHeightDraft.value = value;
  if (!terminalLineHeightInvalid.value) ui.setTerminalLineHeight(value);
}

function restoreTerminalLineHeightDraft() {
  terminalLineHeightDraft.value = String(ui.terminalLineHeight);
}

function updateTerminalLetterSpacing(value: string) {
  terminalLetterSpacingDraft.value = value;
  if (!terminalLetterSpacingInvalid.value) ui.setTerminalLetterSpacing(value);
}

function restoreTerminalLetterSpacingDraft() {
  terminalLetterSpacingDraft.value = String(ui.terminalLetterSpacing);
}

async function refreshVaultAfterNativeChange() {
  vaultStatus.value = await fetchVaultStatus().catch(() => null);
}
onMounted(() => window.addEventListener("norishell:vault-changed", refreshVaultAfterNativeChange));
onBeforeUnmount(() => window.removeEventListener("norishell:vault-changed", refreshVaultAfterNativeChange));

onMounted(async () => {
  const [fonts, status] = await Promise.all([
    detectInstalledTerminalFonts(),
    fetchVaultStatus().catch(() => null),
  ]);
  installedTerminalFonts.value = fonts;
  vaultStatus.value = status;
  vaultStatusLoading.value = false;
  terminalFontsLoading.value = false;
  if (
    ui.terminalFontFamily
    && !installedTerminalFonts.value.includes(ui.terminalFontFamily)
  ) {
    ui.setTerminalFontFamily("");
  }
});

async function openVaultAccess() {
  if (vaultActionLoading.value || vaultStatusLoading.value) return;
  await runSecureVault(
    vaultStatus.value?.state !== "missing" && vaultStatus.value?.unlockPolicy === "automaticLocal"
      ? "unlockSavedLocal"
      : "ensureUnlocked",
  );
}

async function runSecureVault(mode: SecureVaultMode) {
  vaultActionLoading.value = true;
  vaultActionError.value = "";
  try {
    await requestSecureVault(mode);
    vaultStatus.value = await fetchVaultStatus();
  } catch {
    vaultActionError.value = t("sshHosts.vault.failed");
    vaultStatus.value = await fetchVaultStatus().catch(() => null);
  } finally {
    vaultActionLoading.value = false;
  }
}

async function setVaultPolicy(value: string) {
  const repairsFailedAutoUnlock = (
    (value === "automatic" || value === "automaticLocal")
    && value === vaultStatus.value?.unlockPolicy
    && vaultStatus.value?.autoUnlockFailure != null
  );
  if (
    (value === vaultStatus.value?.unlockPolicy && !repairsFailedAutoUnlock)
    || vaultActionLoading.value
  ) return;
  vaultActionError.value = "";
  if (value === "automatic" || value === "automaticLocal") {
    await runSecureVault(value === "automatic" ? "enableAutoUnlock" : "enableLocalAutoUnlock");
    return;
  }
  if (value !== "currentSession") return;

  vaultActionLoading.value = true;
  try {
    vaultStatus.value = await disableVaultAutoUnlock();
  } catch (error) {
    const coreError = parseCoreApiError(error);
    vaultActionError.value = coreError?.messageKey && te(coreError.messageKey)
      ? t(coreError.messageKey)
      : t("sshSettings.vault.errors.policyUpdate");
  } finally {
    vaultActionLoading.value = false;
  }
}

async function lockVaultNow() {
  if (vaultActionLoading.value) return;
  vaultActionLoading.value = true;
  vaultActionError.value = "";
  try {
    vaultStatus.value = await lockVault();
  } catch (error) {
    const coreError = parseCoreApiError(error);
    vaultActionError.value = coreError?.messageKey && te(coreError.messageKey)
      ? t(coreError.messageKey)
      : t("sshSettings.vault.errors.lock");
  } finally {
    vaultActionLoading.value = false;
  }
}

function paletteStyle(palette: TerminalPalette) {
  return {
    background: palette.background,
    color: palette.foreground,
    borderColor: palette.selection,
  };
}

function openCustomEditor() {
  customBase.value = "current";
  customName.value = ui.customTerminalPaletteName
    || t("sshSettings.terminalAppearance.customEditor.defaultName");
  customPalette.value = cloneTerminalPalette(
    ui.hasCustomTerminalPalette
      ? ui.customTerminalPalette
      : ui.resolvedTerminalPalette,
  );
  customError.value = "";
  customEditorOpen.value = true;
}

function setCustomBase(value: string) {
  if (value === "current") {
    customBase.value = value;
    customPalette.value = cloneTerminalPalette(ui.resolvedTerminalPalette);
  } else if (isTerminalPresetId(value)) {
    customBase.value = value;
    customPalette.value = cloneTerminalPalette(terminalPreset(value).palette);
  }
  customError.value = "";
}

function setDraftColor(key: TerminalColorKey, value: string) {
  customPalette.value = {
    ...customPalette.value,
    [key]: value.toLowerCase(),
  };
  customError.value = "";
}

function colorInputValue(key: TerminalColorKey) {
  return isHexColor(customPalette.value[key]) ? customPalette.value[key] : "#000000";
}

function saveCustomScheme() {
  if (!customName.value.trim()) {
    customError.value = "name";
    return;
  }
  const parsedPalette = parseTerminalPalette(customPalette.value);
  if (!parsedPalette) {
    customError.value = "colors";
    return;
  }
  if (!ui.saveCustomTerminalPalette(customName.value, parsedPalette)) {
    customError.value = "storage";
    return;
  }
  customError.value = "";
  customEditorOpen.value = false;
}
</script>

<template>
  <section class="settings-page">
    <div class="settings-shell">
      <aside class="settings-sidebar">
        <h1>{{ t("sshSettings.title") }}</h1>

        <nav :aria-label="t('sshSettings.navigation.label')">
          <section class="settings-sidebar__group">
            <h2>{{ t("sshSettings.navigation.general") }}</h2>
            <button
              class="settings-nav-item"
              :class="{ 'settings-nav-item--active': activeSection === 'application' }"
              type="button"
              :aria-current="activeSection === 'application' ? 'page' : undefined"
              @click="activeSection = 'application'"
            >
              <span
                class="settings-nav-item__icon"
                aria-hidden="true"
              >
                <NvxIcon
                  :icon="Settings"
                  :size="16"
                />
              </span>
              <span>{{ t("sshSettings.applicationPreferences.title") }}</span>
            </button>
            <button
              class="settings-nav-item"
              :class="{ 'settings-nav-item--active': activeSection === 'terminal' }"
              type="button"
              :aria-current="activeSection === 'terminal' ? 'page' : undefined"
              @click="activeSection = 'terminal'"
            >
              <span
                class="settings-nav-item__icon"
                aria-hidden="true"
              >
                <NvxIcon
                  :icon="SquareTerminal"
                  :size="16"
                />
              </span>
              <span>{{ t("sshSettings.navigation.terminal") }}</span>
            </button>
            <button
              v-for="section in enhancementSections"
              :key="section.id"
              class="settings-nav-item"
              :class="{ 'settings-nav-item--active': activeSection === section.id }"
              type="button"
              :aria-current="activeSection === section.id ? 'page' : undefined"
              @click="activeSection = section.id"
            >
              <span
                class="settings-nav-item__icon"
                aria-hidden="true"
              ><NvxIcon
                :icon="section.icon"
                :size="16"
              /></span>
              <span>{{ t(section.label) }}</span>
            </button>
            <button
              class="settings-nav-item"
              :class="{ 'settings-nav-item--active': activeSection === 'about' }"
              type="button"
              :aria-current="activeSection === 'about' ? 'page' : undefined"
              @click="activeSection = 'about'"
            >
              <span
                class="settings-nav-item__icon"
                aria-hidden="true"
              >
                <NvxIcon
                  :icon="Info"
                  :size="16"
                />
              </span>
              <span>{{ t("releases.aboutTitle") }}</span>
              <span
                v-if="appUpdate.hasUpdate"
                class="settings-nav-item__update-badge"
              >{{ t("releases.newBadge") }}</span>
            </button>
          </section>

          <section class="settings-sidebar__group settings-sidebar__group--security">
            <h2>{{ t("sshSettings.navigation.security") }}</h2>
            <button
              class="settings-security-item"
              :class="{ 'settings-security-item--active': activeSection === 'vault' }"
              type="button"
              :aria-current="activeSection === 'vault' ? 'page' : undefined"
              @click="activeSection = 'vault'"
            >
              <span
                class="settings-security-item__icon"
                aria-hidden="true"
              >
                <NvxIcon
                  :icon="LockKeyhole"
                  :size="16"
                />
              </span>
              <span class="settings-security-item__copy">
                <strong>{{ t("sshSettings.vault.title") }}</strong>
                <small :class="`settings-security-item__status settings-security-item__status--${vaultStateTone}`">
                  {{ vaultStateLabel }}
                </small>
              </span>
            </button>
            <button
              v-for="link in securityLinks"
              :key="link.key"
              class="settings-security-item"
              :class="{ 'settings-security-item--active': activeSection === link.section }"
              type="button"
              :aria-label="t(`sshSettings.${link.key}.open`)"
              :aria-current="activeSection === link.section ? 'page' : undefined"
              @click="activeSection = link.section"
            >
              <span
                class="settings-security-item__icon"
                aria-hidden="true"
              >
                <NvxIcon
                  :icon="link.icon"
                  :size="16"
                />
              </span>
              <span class="settings-security-item__copy">
                <strong>{{ t(`sshSettings.${link.key}.title`) }}</strong>
                <small class="settings-security-item__status settings-security-item__status--success">
                  {{ t(`sshSettings.${link.key}.status`) }}
                </small>
              </span>
            </button>
          </section>
        </nav>
      </aside>

      <main class="settings-detail">
        <NvxPluginExtensionTarget
          target-id="settings.tools"
          instance-key="global"
          class="settings-detail__tools"
        />

        <NvxAppThemeSettings v-if="activeSection === 'appearance'" />

        <section
          v-else-if="activeSection === 'application'"
          class="application-preferences"
          aria-labelledby="application-preferences-title"
        >
          <div class="application-preferences__heading">
            <div>
              <h2 id="application-preferences-title">
                {{ t("sshSettings.applicationPreferences.title") }}
              </h2>
              <p>{{ t("sshSettings.applicationPreferences.description") }}</p>
            </div>
          </div>
          <div class="application-preferences__controls">
            <div class="application-preference">
              <span class="application-preference__identity">
                <NvxIcon
                  :icon="Type"
                  :size="20"
                />
                <span class="application-preference__copy">
                  <strong>{{ t("sshSettings.applicationPreferences.zoom") }}</strong>
                  <small>{{ t("sshSettings.applicationPreferences.zoomDescription") }}</small>
                </span>
              </span>
              <NvxSelect
                :model-value="String(ui.uiZoom)"
                :options="uiZoomOptions"
                :disabled="ui.uiZoomBusy"
                :aria-label="t('sshSettings.applicationPreferences.zoom')"
                @update:model-value="setUiZoom"
              />
            </div>
            <div class="application-preference">
              <span class="application-preference__identity">
                <NvxIcon
                  :icon="Globe2"
                  :size="20"
                />
                <span class="application-preference__copy">
                  <strong>{{ t("sshSettings.applicationPreferences.language") }}</strong>
                  <small>{{ t("sshSettings.applicationPreferences.languageDescription") }}</small>
                </span>
              </span>
              <NvxSelect
                :model-value="ui.localePreference"
                :options="localeOptions"
                :aria-label="t('sshSettings.applicationPreferences.language')"
                @update:model-value="setLocale"
              />
            </div>
            <div class="application-preference">
              <span class="application-preference__identity">
                <NvxIcon
                  :icon="Power"
                  :size="20"
                />
                <span class="application-preference__copy">
                  <strong>{{ t("sshSettings.applicationPreferences.startupBehavior") }}</strong>
                  <small>{{ t("sshSettings.applicationPreferences.startupBehaviorDescription") }}</small>
                </span>
              </span>
              <NvxSelect
                :model-value="ui.terminalStartupBehavior"
                :options="terminalStartupBehaviorOptions"
                :aria-label="t('sshSettings.applicationPreferences.startupBehavior')"
                @update:model-value="setTerminalStartupBehavior"
              />
            </div>
            <div class="application-preference">
              <span class="application-preference__identity">
                <NvxIcon
                  :icon="SquareTerminal"
                  :size="20"
                />
                <span class="application-preference__copy">
                  <strong>{{ t("sshSettings.applicationPreferences.newTerminalBehavior") }}</strong>
                  <small>{{ t("sshSettings.applicationPreferences.newTerminalBehaviorDescription") }}</small>
                </span>
              </span>
              <NvxSelect
                :model-value="ui.newTerminalBehavior"
                :options="newTerminalBehaviorOptions"
                :aria-label="t('sshSettings.applicationPreferences.newTerminalBehavior')"
                @update:model-value="setNewTerminalBehavior"
              />
            </div>
            <div class="application-preference">
              <span class="application-preference__identity">
                <NvxIcon
                  :icon="SquareX"
                  :size="20"
                />
                <span class="application-preference__copy">
                  <strong>{{ t("sshSettings.applicationPreferences.singlePaneTabCloseBehavior") }}</strong>
                  <small>{{ t("sshSettings.applicationPreferences.singlePaneTabCloseBehaviorDescription") }}</small>
                </span>
              </span>
              <NvxSelect
                :model-value="ui.singlePaneTabCloseBehavior"
                :options="singlePaneTabCloseBehaviorOptions"
                :aria-label="t('sshSettings.applicationPreferences.singlePaneTabCloseBehavior')"
                @update:model-value="setSinglePaneTabCloseBehavior"
              />
            </div>
          </div>
        </section>

        <NvxReleaseSettings v-else-if="activeSection === 'about'" />
        <NvxPreferenceTransferSettings v-else-if="activeSection === 'transfer'" />
        <div v-else-if="activeSection === 'desktop'">
          <NvxDesktopPreferencesSettings />
          <NvxNativeNotificationSettings />
        </div>
        <NvxTerminalInteractionSettings v-else-if="activeSection === 'interaction'" />
        <NvxSftpSettings v-else-if="activeSection === 'files'" />
        <NvxTerminalEnhancementSettings v-else-if="activeSection === 'enhancements'">
          <NvxNativeShellSettings @saved="nativeTerminal.refresh()" />
        </NvxTerminalEnhancementSettings>
        <section v-else-if="activeSection === 'highlights'">
          <NvxInlineNotice
            v-if="highlightHostsFailed"
            tone="warning"
          >
            {{ t('terminalEnhancements.hostListFailed') }}
            <NvxButton
              variant="ghost"
              @click="loadHighlightHosts"
            >
              {{ t('terminalEnhancements.retry') }}
            </NvxButton>
          </NvxInlineNotice>
          <NvxHighlightSettings :hosts="highlightHosts" />
        </section>
        <NvxShortcutSettings v-else-if="activeSection === 'shortcuts'" />
        <KnownHostsSettingsView v-else-if="activeSection === 'knownHosts'" />
        <IdentitiesSettingsView v-else-if="activeSection === 'identities'" />
        <section
          v-else-if="activeSection === 'terminal'"
          class="terminal-appearance"
          aria-labelledby="terminal-appearance-title"
        >
          <div class="terminal-appearance__heading">
            <div>
              <h2 id="terminal-appearance-title">
                {{ t("sshSettings.terminalAppearance.title") }}
              </h2>
              <p>{{ t("sshSettings.terminalAppearance.description") }}</p>
            </div>
          </div>

          <div class="terminal-scheme-grid">
            <button
              class="terminal-scheme"
              :class="{ 'terminal-scheme--selected': ui.terminalThemeMode === 'follow-app' }"
              type="button"
              :aria-pressed="ui.terminalThemeMode === 'follow-app'"
              @click="selectTerminalScheme('follow-app')"
            >
              <span
                class="terminal-scheme__preview"
                :style="paletteStyle(ui.theme === 'light' ? TERMINAL_PRESETS[0].palette : TERMINAL_PRESETS[1].palette)"
                aria-hidden="true"
              >
                <span class="terminal-scheme__command">$ ssh host</span>
                <span class="terminal-scheme__swatches">
                  <i
                    v-for="color in previewAnsiColors"
                    :key="color"
                    :style="{ background: (ui.theme === 'light' ? TERMINAL_PRESETS[0].palette : TERMINAL_PRESETS[1].palette)[color] }"
                  />
                </span>
              </span>
              <span class="terminal-scheme__copy">
                <strong>{{ t("sshSettings.terminalAppearance.modes.followApp") }}</strong>
                <small>{{ t("sshSettings.terminalAppearance.followAppDescription") }}</small>
              </span>
              <NvxIcon
                v-if="ui.terminalThemeMode === 'follow-app'"
                class="terminal-scheme__check"
                :icon="Check"
                :size="16"
                aria-hidden="true"
              />
            </button>

            <button
              v-for="preset in TERMINAL_PRESETS"
              :key="preset.id"
              class="terminal-scheme"
              :class="{ 'terminal-scheme--selected': ui.terminalThemeMode === preset.id }"
              type="button"
              :aria-pressed="ui.terminalThemeMode === preset.id"
              @click="selectTerminalScheme(preset.id)"
            >
              <span
                class="terminal-scheme__preview"
                :style="paletteStyle(preset.palette)"
                aria-hidden="true"
              >
                <span class="terminal-scheme__command">$ ssh host</span>
                <span class="terminal-scheme__swatches">
                  <i
                    v-for="color in previewAnsiColors"
                    :key="color"
                    :style="{ background: preset.palette[color] }"
                  />
                </span>
              </span>
              <span class="terminal-scheme__copy">
                <strong>{{ t(preset.nameKey) }}</strong>
                <small>{{ t(preset.descriptionKey) }}</small>
              </span>
              <NvxIcon
                v-if="ui.terminalThemeMode === preset.id"
                class="terminal-scheme__check"
                :icon="Check"
                :size="16"
                aria-hidden="true"
              />
            </button>

            <button
              v-if="ui.hasCustomTerminalPalette"
              class="terminal-scheme"
              :class="{ 'terminal-scheme--selected': ui.terminalThemeMode === 'custom' }"
              type="button"
              :aria-pressed="ui.terminalThemeMode === 'custom'"
              @click="selectTerminalScheme('custom')"
            >
              <span
                class="terminal-scheme__preview"
                :style="paletteStyle(ui.customTerminalPalette)"
                aria-hidden="true"
              >
                <span class="terminal-scheme__command">$ ssh host</span>
                <span class="terminal-scheme__swatches">
                  <i
                    v-for="color in previewAnsiColors"
                    :key="color"
                    :style="{ background: ui.customTerminalPalette[color] }"
                  />
                </span>
              </span>
              <span class="terminal-scheme__copy">
                <strong>{{ customDisplayName }}</strong>
                <small>{{ t("sshSettings.terminalAppearance.customSavedDescription") }}</small>
              </span>
              <NvxIcon
                v-if="ui.terminalThemeMode === 'custom'"
                class="terminal-scheme__check"
                :icon="Check"
                :size="16"
                aria-hidden="true"
              />
            </button>
          </div>

          <div class="terminal-typography">
            <span
              class="terminal-typography__icon"
              aria-hidden="true"
            >
              <NvxIcon
                :icon="Type"
                :size="20"
              />
            </span>
            <div class="terminal-typography__body">
              <div class="terminal-typography__controls">
                <NvxField :label="t('sshSettings.terminalAppearance.typography.fontFamily')">
                  <NvxSelect
                    :model-value="ui.terminalFontFamily"
                    :options="terminalFontOptions"
                    :aria-label="t('sshSettings.terminalAppearance.typography.fontFamily')"
                    @update:model-value="setTerminalFontFamily"
                  />
                </NvxField>
                <NvxField
                  for-id="terminal-font-size"
                  :label="t('sshSettings.terminalAppearance.typography.fontSize')"
                  :error="terminalFontSizeInvalid ? t('sshSettings.terminalAppearance.typography.fontSizeError', { min: MIN_TERMINAL_FONT_SIZE, max: MAX_TERMINAL_FONT_SIZE }) : undefined"
                >
                  <NvxInput
                    id="terminal-font-size"
                    :model-value="terminalFontSizeDraft"
                    type="number"
                    :min="MIN_TERMINAL_FONT_SIZE"
                    :max="MAX_TERMINAL_FONT_SIZE"
                    :step="1"
                    :invalid="terminalFontSizeInvalid"
                    @update:model-value="updateTerminalFontSize"
                    @blur="restoreTerminalFontSizeDraft"
                  />
                </NvxField>
                <NvxField :label="t('sshSettings.terminalAppearance.typography.fontWeight')">
                  <NvxSelect
                    :model-value="String(ui.terminalFontWeight)"
                    :options="terminalFontWeightOptions"
                    :aria-label="t('sshSettings.terminalAppearance.typography.fontWeight')"
                    @update:model-value="ui.setTerminalFontWeight"
                  />
                </NvxField>
                <NvxField :label="t('sshSettings.terminalAppearance.typography.boldFontWeight')">
                  <NvxSelect
                    :model-value="String(ui.terminalBoldFontWeight)"
                    :options="terminalFontWeightOptions"
                    :aria-label="t('sshSettings.terminalAppearance.typography.boldFontWeight')"
                    @update:model-value="ui.setTerminalBoldFontWeight"
                  />
                </NvxField>
                <NvxField
                  for-id="terminal-line-height"
                  :label="t('sshSettings.terminalAppearance.typography.lineHeight')"
                  :error="terminalLineHeightInvalid ? t('sshSettings.terminalAppearance.typography.lineHeightError') : undefined"
                >
                  <NvxInput
                    id="terminal-line-height"
                    :model-value="terminalLineHeightDraft"
                    type="number"
                    :min="MIN_TERMINAL_LINE_HEIGHT"
                    :max="MAX_TERMINAL_LINE_HEIGHT"
                    :step="0.1"
                    :invalid="terminalLineHeightInvalid"
                    @update:model-value="updateTerminalLineHeight"
                    @blur="restoreTerminalLineHeightDraft"
                  />
                </NvxField>
                <NvxField
                  for-id="terminal-letter-spacing"
                  :label="t('sshSettings.terminalAppearance.typography.letterSpacing')"
                  :error="terminalLetterSpacingInvalid ? t('sshSettings.terminalAppearance.typography.letterSpacingError') : undefined"
                >
                  <NvxInput
                    id="terminal-letter-spacing"
                    :model-value="terminalLetterSpacingDraft"
                    type="number"
                    :min="MIN_TERMINAL_LETTER_SPACING"
                    :max="MAX_TERMINAL_LETTER_SPACING"
                    :step="1"
                    :invalid="terminalLetterSpacingInvalid"
                    @update:model-value="updateTerminalLetterSpacing"
                    @blur="restoreTerminalLetterSpacingDraft"
                  />
                </NvxField>
                <NvxField :label="t('sshSettings.terminalAppearance.typography.cursorStyle')">
                  <NvxSelect
                    :model-value="ui.terminalCursorStyle"
                    :options="terminalCursorStyleOptions"
                    :aria-label="t('sshSettings.terminalAppearance.typography.cursorStyle')"
                    @update:model-value="ui.setTerminalCursorStyle($event as TerminalCursorStyle)"
                  />
                </NvxField>
                <NvxField :label="t('sshSettings.terminalAppearance.typography.cursorBlink')">
                  <NvxCheckbox
                    :model-value="ui.terminalCursorBlink"
                    @update:model-value="ui.setTerminalCursorBlink"
                  >
                    {{ t("sshSettings.terminalAppearance.typography.cursorBlinkEnabled") }}
                  </NvxCheckbox>
                </NvxField>
              </div>
              <p class="terminal-typography__note">
                {{ t(terminalFontsLoading
                  ? "sshSettings.terminalAppearance.typography.fontFamilyLoading"
                  : "sshSettings.terminalAppearance.typography.configurationHint", {
                  min: MIN_TERMINAL_FONT_SIZE,
                  max: MAX_TERMINAL_FONT_SIZE,
                }) }}
              </p>
            </div>
          </div>

          <div class="terminal-appearance__custom-action">
            <NvxButton
              variant="secondary"
              size="sm"
              @click="openCustomEditor"
            >
              <NvxIcon
                :icon="ui.hasCustomTerminalPalette ? Pencil : Plus"
                :size="16"
                aria-hidden="true"
              />
              {{ t(ui.hasCustomTerminalPalette
                ? "sshSettings.terminalAppearance.editCustom"
                : "sshSettings.terminalAppearance.createCustom") }}
            </NvxButton>
            <p>{{ t("sshSettings.terminalAppearance.customActionHint") }}</p>
          </div>
        </section>

        <article
          v-else-if="activeSection === 'vault'"
          class="settings-vault"
        >
          <div class="settings-vault__heading">
            <div>
              <h2>{{ t("sshSettings.vault.title") }}</h2>
              <p>{{ t("sshSettings.vault.description") }}</p>
            </div>
            <NvxStatusLabel :tone="vaultStateTone">
              {{ vaultStateLabel }}
            </NvxStatusLabel>
          </div>
          <div class="settings-vault__body">
            <div class="vault-controls">
              <NvxField
                :label="t('sshSettings.vault.unlockPolicy')"
                :hint="vaultPolicyHint"
              >
                <NvxSelect
                  :model-value="vaultStatus?.unlockPolicy ?? 'currentSession'"
                  :options="vaultPolicyOptions"
                  :disabled="vaultStatusLoading || !vaultStatus || vaultStatus.state === 'missing' || vaultActionLoading"
                  :aria-label="t('sshSettings.vault.unlockPolicy')"
                  @update:model-value="setVaultPolicy"
                />
              </NvxField>
              <NvxButton
                v-if="vaultStatus?.state !== 'unlocked'"
                size="sm"
                :disabled="vaultStatusLoading || vaultActionLoading"
                @click="openVaultAccess"
              >
                {{ vaultStatus?.state === 'missing' ? t('sshHosts.vault.createAction') : vaultStatus ? t('sshHosts.vault.unlockAction') : t('desktop.refresh') }}
              </NvxButton>
              <NvxButton
                v-if="vaultStatus?.state === 'unlocked'"
                size="sm"
                variant="secondary"
                :loading="vaultActionLoading"
                :loading-label="t('sshSettings.vault.locking')"
                @click="lockVaultNow"
              >
                {{ t("sshSettings.vault.lockNow") }}
              </NvxButton>
            </div>
            <NvxInlineNotice
              v-if="autoUnlockFailureMessage"
              tone="warning"
              :title="t('sshSettings.vault.autoUnlockFallbackTitle')"
            >
              <div class="vault-fallback">
                <span>{{ autoUnlockFailureMessage }}</span>
                <NvxButton
                  size="sm"
                  variant="secondary"
                  @click="setVaultPolicy(autoUnlockRepairPolicy)"
                >
                  {{ t("sshSettings.vault.reenable") }}
                </NvxButton>
              </div>
            </NvxInlineNotice>
            <NvxInlineNotice
              v-if="vaultActionError"
              tone="error"
            >
              {{ vaultActionError }}
            </NvxInlineNotice>
          </div>
          <div class="settings-vault__backup">
            <OfflineBackup />
          </div>
        </article>
      </main>
    </div>

    <NvxDialog
      v-model="customEditorOpen"
      :title="t('sshSettings.terminalAppearance.customEditor.title')"
      :description="t('sshSettings.terminalAppearance.customEditor.description')"
      :close-label="t('sshSettings.terminalAppearance.customEditor.close')"
      @close="customError = ''"
    >
      <div class="custom-editor">
        <div class="custom-editor__essentials">
          <NvxField
            for-id="terminal-custom-name"
            :label="t('sshSettings.terminalAppearance.customEditor.name')"
            :error="customError === 'name' ? customErrorMessage : undefined"
          >
            <NvxInput
              id="terminal-custom-name"
              v-model="customName"
              :maxlength="48"
              :invalid="customError === 'name'"
              data-nvx-dialog-initial-focus
              @update:model-value="customError = ''"
            />
          </NvxField>
          <NvxField
            :label="t('sshSettings.terminalAppearance.customEditor.base')"
            :hint="t('sshSettings.terminalAppearance.customEditor.baseHint')"
          >
            <NvxSelect
              :model-value="customBase"
              :options="customBaseOptions"
              :aria-label="t('sshSettings.terminalAppearance.customEditor.base')"
              @update:model-value="setCustomBase"
            />
          </NvxField>
        </div>

        <div
          class="custom-editor__preview"
          :style="paletteStyle(parseTerminalPalette(customPalette) ?? ui.resolvedTerminalPalette)"
          aria-hidden="true"
        >
          <span><i :style="{ color: customPalette.green }">deploy@host</i>:~$ uptime</span>
          <span :style="{ color: customPalette.muted }">42 days, load average: 0.18</span>
          <span class="custom-editor__preview-swatches">
            <i
              v-for="color in previewAnsiColors"
              :key="color"
              :style="{ background: colorInputValue(color) }"
            />
          </span>
        </div>

        <fieldset
          v-for="group in terminalColorGroups"
          :key="group.labelKey"
          class="terminal-color-group"
        >
          <legend>{{ t(group.labelKey) }}</legend>
          <div class="terminal-color-grid">
            <label
              v-for="color in group.colors"
              :key="color"
              class="terminal-color-field"
            >
              <span>{{ t(`sshSettings.terminalAppearance.colors.${color}`) }}</span>
              <span class="terminal-color-field__control">
                <input
                  type="color"
                  :value="colorInputValue(color)"
                  :aria-label="t(`sshSettings.terminalAppearance.colors.${color}`)"
                  @input="setDraftColor(color, ($event.target as HTMLInputElement).value)"
                >
                <NvxInput
                  :model-value="customPalette[color]"
                  :maxlength="7"
                  :invalid="customError === 'colors' && !isHexColor(customPalette[color])"
                  :aria-label="`${t(`sshSettings.terminalAppearance.colors.${color}`)} HEX`"
                  @update:model-value="setDraftColor(color, $event)"
                />
              </span>
            </label>
          </div>
        </fieldset>

        <p
          v-if="customError === 'colors' || customError === 'storage'"
          class="custom-editor__error"
          role="alert"
        >
          {{ customErrorMessage }}
        </p>
      </div>

      <template #actions>
        <NvxButton
          variant="secondary"
          @click="customEditorOpen = false"
        >
          {{ t("sshSettings.terminalAppearance.customEditor.cancel") }}
        </NvxButton>
        <NvxButton @click="saveCustomScheme">
          {{ t("sshSettings.terminalAppearance.customEditor.save") }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.settings-page {
  height: 100%;
  min-height: 100%;
  padding: 0;
}

.settings-shell {
  display: grid;
  grid-template-columns: 236px minmax(0, 1fr);
  height: 100%;
  min-height: 100%;
  overflow: hidden;
  background: var(--nvx-color-bg-surface);
}

.settings-sidebar {
  min-height: 0;
  padding: var(--nvx-space-5) var(--nvx-space-2) var(--nvx-space-4);
  overflow-y: auto;
  border-right: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-canvas);
}

.settings-sidebar h1 {
  margin: 0 var(--nvx-space-2) var(--nvx-space-5);
  font-size: var(--nvx-font-size-lg);
  line-height: var(--nvx-line-height-lg);
}

.settings-sidebar nav,
.settings-sidebar__group {
  display: grid;
}

.settings-sidebar nav {
  gap: var(--nvx-space-6);
}

.settings-sidebar__group {
  gap: var(--nvx-space-1);
}

.settings-sidebar__group h2 {
  margin: 0 var(--nvx-space-2) var(--nvx-space-1);
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-body);
  line-height: var(--nvx-line-height-body);
}

.settings-nav-item,
.settings-security-item {
  width: 100%;
  border: 0;
  border-radius: var(--nvx-radius-md);
  background: transparent;
  color: var(--nvx-color-text-secondary);
  font: inherit;
  text-align: left;
  cursor: pointer;
  transition: background-color var(--nvx-motion-fast), color var(--nvx-motion-fast), box-shadow var(--nvx-motion-fast);
}

.settings-nav-item:hover,
.settings-security-item:hover {
  background: var(--nvx-color-bg-hover);
  color: var(--nvx-color-text-primary);
}

.settings-nav-item:focus-visible,
.settings-security-item:focus-visible {
  outline: none;
  box-shadow: 0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring);
}

.settings-nav-item {
  display: grid;
  grid-template-columns: 32px minmax(0, 1fr) auto;
  gap: var(--nvx-space-1);
  align-items: center;
  min-height: 40px;
  padding: var(--nvx-space-1) var(--nvx-space-2);
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-medium);
}

.settings-nav-item--active,
.settings-security-item--active {
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}

.settings-nav-item__update-badge {
  padding: 2px 5px;
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-accent);
  color: #fff;
  font-size: 10px;
  line-height: 1;
  font-weight: 700;
}

.settings-nav-item--active:hover,
.settings-security-item--active:hover {
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}

.settings-nav-item__icon,
.settings-security-item__icon {
  display: grid;
  width: 32px;
  height: 32px;
  place-items: center;
}

.settings-security-item {
  display: grid;
  grid-template-columns: 32px minmax(0, 1fr);
  gap: var(--nvx-space-1);
  align-items: center;
  min-height: 52px;
  padding: var(--nvx-space-1) var(--nvx-space-2);
}

.settings-security-item__icon {
  align-self: center;
  color: currentColor;
}

.settings-security-item__copy {
  display: grid;
  gap: 1px;
  min-width: 0;
}

.settings-security-item__copy strong {
  overflow: hidden;
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-medium);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.settings-security-item--active .settings-security-item__copy strong {
  color: var(--nvx-color-accent);
}

.settings-security-item__status {
  display: flex;
  gap: var(--nvx-space-1);
  align-items: center;
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
}

.settings-security-item__status::before {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
  content: "";
}

.settings-security-item__status--success {
  color: var(--nvx-color-success);
}

.settings-security-item__status--warning {
  color: var(--nvx-color-warning);
}

.settings-security-item__status--neutral {
  color: var(--nvx-color-text-tertiary);
}

.settings-detail {
  position: relative;
  min-height: 0;
  min-width: 0;
  padding: var(--nvx-space-6) var(--nvx-space-8) var(--nvx-space-5);
  overflow-y: auto;
}

.settings-detail__tools:empty {
  display: none;
}

.settings-detail__tools {
  position: absolute;
  top: var(--nvx-space-6);
  right: var(--nvx-space-8);
}

.application-preferences,
.terminal-appearance,
.settings-vault {
  min-width: 0;
}

.application-preferences__heading,
.terminal-appearance__heading {
  padding-bottom: var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.application-preferences h2,
.application-preferences p,
.terminal-appearance h2,
.terminal-appearance p {
  margin: 0;
}

.application-preferences h2,
.terminal-appearance h2 {
  font-size: var(--nvx-font-size-lg);
  line-height: var(--nvx-line-height-lg);
}

.application-preferences p,
.terminal-appearance p {
  margin-top: var(--nvx-space-1);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.application-preferences__controls {
  display: grid;
  margin-top: var(--nvx-space-2);
}

.application-preferences__controls .application-preference {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(220px, 260px);
  gap: var(--nvx-space-5);
  align-items: center;
  min-height: 78px;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.application-preference__identity {
  display: grid;
  grid-template-columns: 40px minmax(0, 1fr);
  gap: var(--nvx-space-4);
  align-items: center;
}

.application-preference__identity > :first-child {
  box-sizing: border-box;
  width: 40px;
  height: 40px;
  padding: 10px;
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-text-secondary);
}

.application-preference__copy {
  display: grid;
  gap: 2px;
  min-width: 0;
}

.application-preference__copy strong {
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-body);
  line-height: var(--nvx-line-height-body);
}

.application-preference__copy small {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
}

.terminal-scheme-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: var(--nvx-space-2);
  margin-top: var(--nvx-space-4);
}

.terminal-scheme {
  position: relative;
  display: grid;
  grid-template-columns: 96px minmax(0, 1fr);
  gap: var(--nvx-space-2);
  min-height: 76px;
  padding: var(--nvx-space-1);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  text-align: left;
  cursor: pointer;
  transition: border-color var(--nvx-motion-fast), background-color var(--nvx-motion-fast), box-shadow var(--nvx-motion-fast);
}

.terminal-scheme:hover {
  border-color: var(--nvx-color-border-strong);
  background: var(--nvx-color-bg-hover);
}

.terminal-scheme:focus-visible {
  outline: none;
  box-shadow: 0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring);
}

.terminal-scheme--selected {
  border-color: var(--nvx-color-accent);
}

.terminal-scheme__preview {
  display: grid;
  align-content: space-between;
  min-width: 0;
  min-height: 64px;
  padding: var(--nvx-space-2);
  overflow: hidden;
  border: var(--nvx-border-width) solid;
  border-radius: var(--nvx-radius-sm);
  font-family: var(--nvx-font-mono);
  font-size: 10px;
}

.terminal-scheme__command {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.terminal-scheme__swatches,
.custom-editor__preview-swatches {
  display: grid;
  grid-template-columns: repeat(6, minmax(0, 1fr));
  gap: 2px;
}

.terminal-scheme__swatches i,
.custom-editor__preview-swatches i {
  height: 6px;
  border-radius: 1px;
}

.terminal-scheme__copy {
  display: grid;
  align-content: center;
  min-width: 0;
}

.terminal-scheme__copy strong,
.terminal-scheme__copy small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.terminal-scheme__copy strong {
  padding-right: var(--nvx-space-5);
  font-size: var(--nvx-font-size-sm);
}

.terminal-scheme__copy small {
  margin-top: var(--nvx-space-1);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.terminal-scheme__check {
  position: absolute;
  top: var(--nvx-space-2);
  right: var(--nvx-space-2);
  color: var(--nvx-color-accent);
}

.terminal-appearance__custom-action {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  margin-top: var(--nvx-space-3);
  padding-top: var(--nvx-space-3);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.terminal-typography {
  display: grid;
  grid-template-columns: 36px minmax(0, 1fr);
  gap: var(--nvx-space-3);
  align-items: start;
  margin-top: var(--nvx-space-3);
  padding-top: var(--nvx-space-3);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.terminal-typography__body {
  width: 100%;
  min-width: 0;
}

.terminal-typography__controls {
  display: grid;
  grid-template-columns: repeat(4, minmax(150px, 1fr));
  gap: var(--nvx-space-3);
  align-items: start;
}

.terminal-typography__controls :deep(.nvx-field__label) {
  min-height: var(--nvx-line-height-sm);
  line-height: var(--nvx-line-height-sm);
}

.terminal-typography__note {
  margin: var(--nvx-space-3) 0 0;
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
}

.terminal-typography__icon {
  display: grid;
  width: 36px;
  height: 36px;
  place-items: center;
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}

.terminal-appearance__custom-action p {
  margin: 0;
  font-size: var(--nvx-font-size-xs);
}

.custom-editor {
  display: grid;
  gap: var(--nvx-space-4);
}

.custom-editor__essentials {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--nvx-space-3);
}

.custom-editor__preview {
  display: grid;
  gap: var(--nvx-space-1);
  min-height: 96px;
  padding: var(--nvx-space-3);
  border: var(--nvx-border-width) solid;
  border-radius: var(--nvx-radius-md);
  font-family: var(--nvx-font-mono);
  font-size: var(--nvx-font-size-xs);
}

.custom-editor__preview i {
  font-style: normal;
}

.custom-editor__preview-swatches {
  align-self: end;
}

.terminal-color-group {
  min-width: 0;
  margin: 0;
  padding: 0;
  border: 0;
}

.terminal-color-group legend {
  margin-bottom: var(--nvx-space-2);
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-medium);
}

.terminal-color-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--nvx-space-2);
}

.terminal-color-field {
  display: grid;
  gap: var(--nvx-space-1);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.terminal-color-field__control {
  display: grid;
  grid-template-columns: 34px minmax(0, 1fr);
  gap: var(--nvx-space-1);
  align-items: center;
}

.terminal-color-field input[type="color"] {
  width: 34px;
  height: var(--nvx-control-height-md);
  padding: 3px;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  cursor: pointer;
}

.terminal-color-field input[type="color"]:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 2px;
}

.custom-editor__error {
  margin: 0;
  color: var(--nvx-color-danger);
  font-size: var(--nvx-font-size-sm);
}

.settings-vault__heading {
  display: flex;
  gap: var(--nvx-space-4);
  align-items: flex-start;
  justify-content: space-between;
  padding-bottom: var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.settings-vault__heading h2,
.settings-vault__heading p {
  margin: 0;
}

.settings-vault__heading h2 {
  font-size: var(--nvx-font-size-lg);
  line-height: var(--nvx-line-height-lg);
}

.settings-vault__heading p {
  margin-top: var(--nvx-space-1);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.settings-vault__body {
  display: grid;
  gap: var(--nvx-space-3);
  padding-top: var(--nvx-space-4);
}

.settings-vault__backup {
  margin-top: var(--nvx-space-6);
  padding-top: var(--nvx-space-6);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.vault-controls {
  display: flex;
  gap: var(--nvx-space-4);
  align-items: center;
  justify-content: space-between;
}

.vault-controls :deep(.nvx-field) {
  width: min(100%, 360px);
}

.vault-enable-form {
  display: grid;
  gap: var(--nvx-space-3);
}

.vault-fallback {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  justify-content: space-between;
}

@media (max-width: 1180px) {
  .settings-shell {
    grid-template-columns: 220px minmax(0, 1fr);
  }

  .settings-detail {
    padding-right: var(--nvx-space-6);
    padding-left: var(--nvx-space-6);
  }

  .terminal-scheme-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 900px) {
  .settings-shell {
    grid-template-columns: 1fr;
  }

  .settings-sidebar {
    padding: var(--nvx-space-4);
    border-right: 0;
    border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  }

  .settings-sidebar h1 {
    margin-bottom: var(--nvx-space-4);
  }

  .settings-sidebar nav {
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--nvx-space-4);
  }

  .settings-detail {
    padding: var(--nvx-space-5);
  }

  .application-preferences__controls .application-preference {
    grid-template-columns: minmax(0, 1fr) minmax(200px, 260px);
  }

  .terminal-scheme-grid {
    grid-template-columns: 1fr;
  }

  .terminal-typography {
    grid-template-columns: 36px minmax(0, 1fr);
  }

  .terminal-typography__controls {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 620px) {
  .settings-sidebar nav {
    grid-template-columns: 1fr;
  }

  .settings-detail {
    padding: var(--nvx-space-4);
  }

  .application-preferences__controls .application-preference {
    grid-template-columns: 1fr;
    gap: var(--nvx-space-3);
    padding: var(--nvx-space-4) 0;
  }

  .terminal-typography__controls {
    grid-template-columns: 1fr;
  }

  .settings-vault__heading,
  .vault-controls {
    align-items: stretch;
    flex-direction: column;
  }
}
</style>
