import { createPinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { cloneDefaultAppThemeProfile } from "./app-theme";
import { initializeApplicationPreferences } from "./application-preferences-startup";
import type { ApplicationPreferenceGroupId, ApplicationPreferencesSnapshot } from "./core-api/generated/core-api";
import { APP_THEME_PREFERENCES_KEY, useAppThemeStore } from "./stores/appTheme";
import { SFTP_PREFERENCES_KEY, useSftpPreferencesStore } from "./stores/sftpPreferences";
import { SECURE_WINDOW_APPEARANCE_KEY } from "./secure-window-appearance";
import { TERMINAL_PREFERENCES_KEY, useTerminalPreferencesStore } from "./stores/terminalPreferences";
import { useUiStore } from "./stores/ui";
import { UI_PREFERENCES_KEY } from "./ui-preferences";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke }));

const snapshots = new Map<ApplicationPreferenceGroupId, ApplicationPreferencesSnapshot>();
let rejectWrites = false;

beforeEach(() => {
  localStorage.clear();
  snapshots.clear();
  rejectWrites = false;
  invoke.mockReset();
  invoke.mockImplementation(async (command: string, args: { request: { group: ApplicationPreferenceGroupId; expectedRevision?: string | null; value?: unknown } }) => {
    const { group } = args.request;
    if (command === "application_preferences_get") return snapshots.get(group) ?? { group, revision: null, value: null };
    if (command !== "application_preferences_replace") throw new Error("unexpected command");
    if (rejectWrites) throw new Error("application_preferences.conflict");
    const previous = snapshots.get(group);
    if ((previous?.revision ?? null) !== args.request.expectedRevision) throw new Error("application_preferences.conflict");
    const revision = String(Number(previous?.revision ?? "0") + 1);
    const next = { group, revision, value: args.request.value };
    snapshots.set(group, next);
    return next;
  });
});

describe("Core preference startup migration", () => {
  it("migrates six explicit groups and preserves separate local settings", async () => {
    const originalUi = JSON.stringify({ themePreference: "dark", locale: "en", terminalFontSize: 16 });
    const appTheme = cloneDefaultAppThemeProfile();
    const originalTheme = JSON.stringify(appTheme);
    const originalTerminal = JSON.stringify({ version: 1, pasteWarning: "always", hostHighlights: {}, hostKeyboard: {
      "host-a": { mode: "override", keyboard: { optionAsMetaLeft: true, optionAsMetaRight: false, backspaceMode: "bs" } },
    } });
    const originalFiles = JSON.stringify({ version: 1, browser: { showHidden: false, foldersFirst: true, sort: "name" },
      rememberLastDirectory: true, directories: { local: "/Users/test", remote: [] } });
    localStorage.setItem(UI_PREFERENCES_KEY, originalUi);
    localStorage.setItem(APP_THEME_PREFERENCES_KEY, originalTheme);
    localStorage.setItem(TERMINAL_PREFERENCES_KEY, originalTerminal);
    localStorage.setItem(SFTP_PREFERENCES_KEY, originalFiles);

    const pinia = createPinia();
    await initializeApplicationPreferences(pinia);
    expect([...snapshots.keys()].sort()).toEqual(["appearance", "application", "files", "highlights", "interaction", "shortcuts"]);
    expect(snapshots.get("application")?.value).toMatchObject({ themePreference: "dark", locale: "en" });
    expect(snapshots.get("appearance")?.value).toMatchObject({ terminalFontSize: 16, appTheme });
    expect(snapshots.get("interaction")?.value).toMatchObject({ pasteWarning: "always" });
    expect(snapshots.get("files")?.value).toMatchObject({ browser: { showHidden: false }, rememberLastDirectory: true });
    expect(localStorage.getItem(UI_PREFERENCES_KEY)).toBe(originalUi);
    expect(localStorage.getItem(APP_THEME_PREFERENCES_KEY)).toBe(originalTheme);
    expect(localStorage.getItem(TERMINAL_PREFERENCES_KEY)).toBe(originalTerminal);
    expect(localStorage.getItem(SFTP_PREFERENCES_KEY)).toBe(originalFiles);
    expect(JSON.parse(localStorage.getItem(SECURE_WINDOW_APPEARANCE_KEY) ?? "{}")).toEqual({ locale: "en", themePreference: "dark" });
    expect(useTerminalPreferencesStore(pinia).resolvedKeyboard("host-a").optionAsMetaLeft).toBe(true);
    expect(useSftpPreferencesStore(pinia).rememberedLocalDirectory()).toBe("/Users/test");
    expect(useAppThemeStore(pinia).profile).toEqual(appTheme);
  });

  it("uses an existing Core value and leaves Pinia unchanged when Core rejects a write", async () => {
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ themePreference: "light", locale: "en" }));
    snapshots.set("application", { group: "application", revision: "7", value: {
      themePreference: "dark", locale: "zh-CN", uiZoom: 100, terminalStartupBehavior: "restoreHistory",
      newTerminalBehavior: "welcome", singlePaneTabCloseBehavior: "confirm",
    } });
    const pinia = createPinia();
    await initializeApplicationPreferences(pinia);
    const ui = useUiStore(pinia);
    expect(ui.themePreference).toBe("dark");
    expect(ui.localePreference).toBe("zh-CN");
    rejectWrites = true;
    await expect(ui.setThemePreference("light")).rejects.toThrow("application_preferences.conflict");
    expect(ui.themePreference).toBe("dark");
    expect(snapshots.get("application")?.revision).toBe("7");
  });
});
