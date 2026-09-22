import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createPreferenceAdapters } from "./preference-adapters";
import { applyPreferencePreview, exportPreferenceTransfer, parsePreferenceTransfer, previewPreferenceTransfer } from "./preferences-transfer";
import { useSftpPreferencesStore } from "./stores/sftpPreferences";
import { useTerminalPreferencesStore } from "./stores/terminalPreferences";
import { useUiStore } from "./stores/ui";
import { DEFAULT_HIGHLIGHT_RULES } from "./terminal/highlighting";

vi.mock("./ui-zoom", async (original) => ({ ...await original<typeof import("./ui-zoom")>(), applyUiZoom: vi.fn(async () => {}) }));
vi.mock("./core-api/client", () => ({ setPluginLocale: vi.fn(async () => {}) }));
vi.mock("./core-api/desktop-preferences", () => ({ getDesktopPreferences: vi.fn(), replaceDesktopPreferences: vi.fn() }));
vi.mock("./core-api/native-terminal", () => ({ getNativeTerminalSettings: vi.fn(), replaceNativeTerminalSettings: vi.fn() }));

describe("global preference adapters", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
    setActivePinia(createPinia());
  });

  it("exports global values without host maps or remembered directory data", async () => {
    const terminal = useTerminalPreferencesStore();
    terminal.setHighlights({ enabled: true, rules: [...DEFAULT_HIGHLIGHT_RULES] }, "host-secret-label");
    const files = useSftpPreferencesStore();
    files.setRememberLastDirectory(true);
    files.rememberLocalDirectory("/private/example");
    files.rememberRemoteDirectory("host-secret-label", [47, 120]);
    const adapters = createPreferenceAdapters().filter((item) => !["desktop", "commandNotifications"].includes(item.id));
    const text = await exportPreferenceTransfer(adapters);
    expect(text).not.toContain("host-secret-label");
    expect(text).not.toContain("/private/example");
    expect(text).not.toContain("pathBytes");
    const groups = parsePreferenceTransfer(text, adapters).groups;
    expect(Object.keys(groups)).toHaveLength(6);
    expect(groups.interaction).toMatchObject({ interaction: { sshReconnectOnInput: true } });
    expect(adapters.find((item) => item.id === "interaction")?.defaults()).toMatchObject({ interaction: { sshReconnectOnInput: true } });
  });

  it("rejects extra fields rather than silently accepting secrets alongside valid settings", async () => {
    const adapters = createPreferenceAdapters();
    for (const adapter of adapters) {
      expect(adapter.validate(adapter.defaults()), adapter.id).toBe(true);
      expect(adapter.validate({ ...(adapter.defaults() as object), credential: "not-exportable" }), adapter.id).toBe(false);
    }
    const highlighter = adapters.find((item) => item.id === "highlights")!;
    expect(highlighter.validate({ enabled: true, rules: [{ ...DEFAULT_HIGHLIGHT_RULES[0], password: "bad" }] })).toBe(false);
  });

  it("applies an appearance group atomically and keeps application and host settings", async () => {
    const ui = useUiStore();
    ui.setThemePreference("dark");
    const terminal = useTerminalPreferencesStore();
    terminal.setHighlights({ enabled: true, rules: [...DEFAULT_HIGHLIGHT_RULES] }, "host-one");
    const adapters = createPreferenceAdapters();
    const value = { ...ui.appearancePreferences(), terminalFontSize: 19, terminalCursorBlink: false };
    const preview = await previewPreferenceTransfer({ product: "NoriShell", version: 1, groups: { appearance: value } }, adapters);
    await applyPreferencePreview(preview, new Set(["appearance"]), adapters);
    expect(preview[0]?.result).toBe("applied");
    expect(ui.terminalFontSize).toBe(19);
    expect(ui.terminalCursorBlink).toBe(false);
    expect(ui.themePreference).toBe("dark");
    expect(terminal.preferences.hostHighlights["host-one"]?.enabled).toBe(true);
  });

  it("reports a failed group and still applies an independent selected group", async () => {
    const ui = useUiStore();
    const adapters = createPreferenceAdapters();
    const files = useSftpPreferencesStore();
    const preview = await previewPreferenceTransfer({ product: "NoriShell", version: 1, groups: {
      appearance: { ...ui.appearancePreferences(), terminalFontSize: 20 },
      files: { browser: { ...files.browser, showHidden: false }, rememberLastDirectory: false },
    } }, adapters);
    vi.spyOn(localStorage, "setItem").mockImplementationOnce(() => { throw new Error("quota"); });
    await applyPreferencePreview(preview, new Set(["appearance", "files"]), adapters);
    expect(preview.map((group) => group.result)).toEqual(["failed", "applied"]);
    expect(ui.terminalFontSize).toBe(13);
    expect(files.browser.showHidden).toBe(false);
  });
});
