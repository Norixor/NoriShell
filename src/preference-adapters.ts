import { isTauri } from "@tauri-apps/api/core";
import { resolveLocale } from "./locales";

import { setPluginLocale } from "./core-api/client";
import { getDesktopPreferences, replaceDesktopPreferences } from "./core-api/desktop-preferences";
import { getNativeTerminalSettings, replaceNativeTerminalSettings } from "./core-api/native-terminal";
import type { DesktopPreferences } from "./core-api/generated/core-api";
import { preferenceValuesEqual, type PreferenceGroupAdapter } from "./preferences-transfer";
import { createDefaultShortcutBindings, parseShortcutProfile, type ShortcutProfile } from "./shortcuts";
import { useDesktopPreferencesStore } from "./stores/desktopPreferences";
import { useNativeTerminalStore } from "./stores/nativeTerminal";
import { DEFAULT_SFTP_PREFERENCES, useSftpPreferencesStore, validSftpBrowserPreferences } from "./stores/sftpPreferences";
import { useShortcutsStore } from "./stores/shortcuts";
import { useTerminalPreferencesStore, type TerminalGlobalInteractionPreferences } from "./stores/terminalPreferences";
import { useUiStore } from "./stores/ui";
import { DEFAULT_HIGHLIGHT_RULES, validateHighlightConfiguration } from "./terminal/highlighting";
import { DEFAULT_TERMINAL_INTERACTION, validateInteractionPreferences } from "./terminal/interaction-preferences";
import {
  DEFAULT_APPLICATION_PREFERENCES, defaultAppearancePreferences, exactPreferenceKeys,
  validateAppearancePreferences, validateApplicationPreferences,
  type ApplicationPreferences, type AppearancePreferences,
} from "./ui-transfer";

const desktopDefaults: DesktopPreferences = {
  windowCloseBehavior: "hide", trayShowStatus: true, trayRecentLimit: 5, trayShowHostNames: true,
  notificationBackgroundOnly: true, notificationFailureOnly: false,
  notifyTransferCompleted: false, notifyTransferFailed: false, notifyDisconnected: false,
};
function validateDesktop(value: unknown): value is DesktopPreferences {
  if (!exactPreferenceKeys(value, Object.keys(desktopDefaults))) return false;
  return ["hide", "quit"].includes(value.windowCloseBehavior as string)
    && typeof value.trayRecentLimit === "number" && Number.isInteger(value.trayRecentLimit) && value.trayRecentLimit >= 0 && value.trayRecentLimit <= 10
    && Object.keys(desktopDefaults).filter((key) => !["windowCloseBehavior", "trayRecentLimit"].includes(key)).every((key) => typeof value[key] === "boolean");
}
function validateGlobalInteraction(value: unknown): value is TerminalGlobalInteractionPreferences {
  return exactPreferenceKeys(value, ["interaction", "pasteWarning"])
    && validateInteractionPreferences(value.interaction) && ["always", "multiline", "never"].includes(value.pasteWarning as string);
}
function validateHighlights(value: unknown) {
  return exactPreferenceKeys(value, ["enabled", "rules"]) && validateHighlightConfiguration(value)
    && value.rules.every((rule) => exactPreferenceKeys(rule, ["id", "label", "pattern", "mode", "caseSensitive", "foreground", "background", "enabled"]));
}
function validateShortcuts(value: unknown): value is ShortcutProfile {
  const parsed = parseShortcutProfile(JSON.stringify(value));
  return parsed.ok && preferenceValuesEqual(value, parsed.profile);
}
interface FilePreferences { browser: typeof DEFAULT_SFTP_PREFERENCES; rememberLastDirectory: boolean }
function validateFiles(value: unknown): value is FilePreferences {
  return exactPreferenceKeys(value, ["browser", "rememberLastDirectory"])
    && validSftpBrowserPreferences(value.browser) && typeof value.rememberLastDirectory === "boolean";
}
interface CommandNotifications { notificationsEnabled: boolean; notificationThresholdSeconds: number }
function validateCommandNotifications(value: unknown): value is CommandNotifications {
  return exactPreferenceKeys(value, ["notificationsEnabled", "notificationThresholdSeconds"])
    && typeof value.notificationsEnabled === "boolean" && typeof value.notificationThresholdSeconds === "number"
    && Number.isInteger(value.notificationThresholdSeconds) && value.notificationThresholdSeconds >= 1 && value.notificationThresholdSeconds <= 3600;
}
const commandValues = (settings: CommandNotifications): CommandNotifications => ({
  notificationsEnabled: settings.notificationsEnabled, notificationThresholdSeconds: settings.notificationThresholdSeconds,
});

export function createPreferenceAdapters(): PreferenceGroupAdapter[] {
  const ui = useUiStore();
  const terminal = useTerminalPreferencesStore();
  const files = useSftpPreferencesStore();
  const shortcuts = useShortcutsStore();
  const desktop = useDesktopPreferencesStore();
  const nativeTerminal = useNativeTerminalStore();
  return [
    {
      id: "application", validate: validateApplicationPreferences, read: async () => ui.applicationPreferences(),
      defaults: () => ({ ...DEFAULT_APPLICATION_PREFERENCES }),
      apply: async (value, expected) => {
        if (!validateApplicationPreferences(value) || !validateApplicationPreferences(expected)
          || !preferenceValuesEqual(ui.applicationPreferences(), expected)) return false;
        const localeChanged = value.locale !== expected.locale && isTauri();
        if (localeChanged) await setPluginLocale(resolveLocale(value.locale));
        const success = await ui.replaceApplicationPreferences(value, expected as ApplicationPreferences);
        if (!success && localeChanged) await setPluginLocale(resolveLocale(expected.locale));
        return success;
      },
    },
    {
      id: "appearance", validate: validateAppearancePreferences, read: async () => ui.appearancePreferences(), defaults: defaultAppearancePreferences,
      apply: async (value, expected) => ui.replaceAppearancePreferences(value, expected as AppearancePreferences),
    },
    {
      id: "interaction", validate: validateGlobalInteraction,
      read: async () => ({ interaction: { ...terminal.preferences.interaction }, pasteWarning: terminal.preferences.pasteWarning }),
      defaults: () => ({ interaction: { ...DEFAULT_TERMINAL_INTERACTION }, pasteWarning: "multiline" }),
      apply: async (value, expected) => terminal.replaceGlobalInteraction(value as TerminalGlobalInteractionPreferences, expected as TerminalGlobalInteractionPreferences),
    },
    {
      id: "highlights", validate: validateHighlights,
      read: async () => JSON.parse(JSON.stringify(terminal.preferences.highlights)),
      defaults: () => ({ enabled: false, rules: DEFAULT_HIGHLIGHT_RULES.map((rule) => ({ ...rule })) }),
      apply: async (value, expected) => validateHighlightConfiguration(value) && preferenceValuesEqual(terminal.preferences.highlights, expected) && terminal.setHighlights(value),
    },
    {
      id: "shortcuts", validate: validateShortcuts, read: async () => JSON.parse(shortcuts.exportProfile()),
      defaults: () => ({ version: 1, bindings: { macos: createDefaultShortcutBindings("macos"), windows: createDefaultShortcutBindings("windows") } }),
      apply: async (value, expected) => preferenceValuesEqual(shortcuts.profile, expected) && (await shortcuts.importProfile(JSON.stringify(value))).ok,
    },
    {
      id: "files", validate: validateFiles, read: async () => files.generalPreferences(),
      defaults: () => ({ browser: { ...DEFAULT_SFTP_PREFERENCES }, rememberLastDirectory: false }),
      apply: async (value, expected) => files.replaceGeneralPreferences(value as FilePreferences, expected as FilePreferences),
    },
    {
      id: "desktop", validate: validateDesktop, read: async () => (await getDesktopPreferences()).preferences, defaults: () => ({ ...desktopDefaults }),
      apply: async (value, expected) => {
        if (!validateDesktop(value)) return false;
        const current = await getDesktopPreferences();
        if (!preferenceValuesEqual(current.preferences, expected)) return false;
        desktop.snapshot = await replaceDesktopPreferences(value, current.revision);
        return true;
      },
    },
    {
      id: "commandNotifications", validate: validateCommandNotifications,
      read: async () => commandValues((await getNativeTerminalSettings()).settings),
      defaults: () => ({ notificationsEnabled: false, notificationThresholdSeconds: 60 }),
      apply: async (value, expected) => {
        if (!validateCommandNotifications(value)) return false;
        const current = await getNativeTerminalSettings();
        if (!preferenceValuesEqual(commandValues(current.settings), expected)) return false;
        const updated = await replaceNativeTerminalSettings({ ...current.settings, ...value }, current.settingsRevision);
        nativeTerminal.applySettings(updated);
        return true;
      },
    },
  ];
}
