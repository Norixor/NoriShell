import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import { useUiStore } from "./stores/ui";
import {
  DARK_TERMINAL_PALETTE,
  DEFAULT_TERMINAL_FONT_FAMILY,
  DEFAULT_TERMINAL_LINE_HEIGHT,
  DEFAULT_TERMINAL_FONT_SIZE,
  LIGHT_TERMINAL_PALETTE,
  parseTerminalPalette,
  parseStoredTerminalAppearance,
  parseTerminalFontSize,
  parseTerminalFontWeight,
  parseTerminalLineHeight,
  parseTerminalLetterSpacing,
  parseTerminalCursorStyle,
  resolveTerminalPalette,
  TERMINAL_COLOR_KEYS,
  TERMINAL_PRESETS,
  terminalFontCssFamily,
  normalizeTerminalFontFamily,
} from "./terminal-theme";

function relativeLuminance(color: string) {
  const channels = [1, 3, 5].map((offset) =>
    Number.parseInt(color.slice(offset, offset + 2), 16) / 255,
  ).map((channel) =>
    channel <= 0.04045
      ? channel / 12.92
      : ((channel + 0.055) / 1.055) ** 2.4,
  );
  return channels[0]! * 0.2126 + channels[1]! * 0.7152 + channels[2]! * 0.0722;
}

function contrastRatio(first: string, second: string) {
  const firstLuminance = relativeLuminance(first);
  const secondLuminance = relativeLuminance(second);
  return (Math.max(firstLuminance, secondLuminance) + 0.05)
    / (Math.min(firstLuminance, secondLuminance) + 0.05);
}

describe("terminal appearance", () => {
  beforeEach(() => {
    localStorage.clear();
    setActivePinia(createPinia());
  });

  it("resolves follow-app, legacy pinned, preset, and custom palettes independently", () => {
    const custom = { ...DARK_TERMINAL_PALETTE, background: "#123456" };

    expect(resolveTerminalPalette("follow-app", "light", custom)).toBe(
      LIGHT_TERMINAL_PALETTE,
    );
    expect(resolveTerminalPalette("follow-app", "dark", custom)).toBe(
      DARK_TERMINAL_PALETTE,
    );
    expect(resolveTerminalPalette("light", "dark", custom)).toBe(
      LIGHT_TERMINAL_PALETTE,
    );
    expect(resolveTerminalPalette("solarized-dark", "light", custom)).toBe(
      TERMINAL_PRESETS.find((preset) => preset.id === "solarized-dark")?.palette,
    );
    expect(resolveTerminalPalette("custom", "light", custom)).toBe(custom);
  });

  it("rejects incomplete or malformed persisted palettes", () => {
    expect(parseTerminalPalette({ background: "#ffffff" })).toBeNull();
    expect(parseTerminalPalette({
      ...DARK_TERMINAL_PALETTE,
      cursor: "not-a-color",
    })).toBeNull();
    expect(parseTerminalPalette(DARK_TERMINAL_PALETTE)).toEqual(
      DARK_TERMINAL_PALETTE,
    );
    expect(parseTerminalPalette({
      ...DARK_TERMINAL_PALETTE,
      cursor: "#AABBCC",
    })?.cursor).toBe("#aabbcc");
  });

  it("ships complete presets whose terminal text roles meet WCAG AA contrast", () => {
    for (const preset of TERMINAL_PRESETS) {
      expect(parseTerminalPalette(preset.palette)).not.toBeNull();
      expect(contrastRatio(preset.palette.background, preset.palette.foreground)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(preset.palette.background, preset.palette.muted)).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("migrates v2 pinned and custom selections without losing the custom palette", () => {
    const custom = { ...DARK_TERMINAL_PALETTE, background: "#123456" };

    expect(parseStoredTerminalAppearance({
      terminalAppearanceVersion: 2,
      terminalThemeMode: "light",
      customTerminalPalette: custom,
    })).toMatchObject({
      mode: "norishell-light",
      customPalette: custom,
      hasCustomPalette: false,
    });
    expect(parseStoredTerminalAppearance({
      terminalAppearanceVersion: 2,
      terminalThemeMode: "custom",
      customTerminalPalette: custom,
    })).toMatchObject({
      mode: "custom",
      customPalette: custom,
      hasCustomPalette: true,
    });
  });

  it("falls back from an invalid v3 preset while retaining a valid saved custom scheme", () => {
    const custom = { ...LIGHT_TERMINAL_PALETTE, cursor: "#123456" };

    expect(parseStoredTerminalAppearance({
      terminalAppearanceVersion: 3,
      terminalThemeMode: "missing-preset",
      customTerminalPalette: custom,
      customTerminalPaletteName: "  Operations  ",
    })).toEqual({
      mode: "follow-app",
      customPalette: custom,
      customName: "Operations",
      hasCustomPalette: true,
    });
  });

  it("selects and persists a fixed preset independently from the app theme", () => {
    const store = useUiStore();
    store.setTerminalThemeMode("nord");
    store.setTheme("light");

    expect(store.terminalThemeMode).toBe("nord");
    expect(store.resolvedTerminalPalette).toEqual(
      TERMINAL_PRESETS.find((preset) => preset.id === "nord")?.palette,
    );
    const persisted = JSON.parse(
      localStorage.getItem("norishell.ui.preferences.v1") ?? "{}",
    ) as { terminalAppearanceVersion?: number; terminalThemeMode?: string };
    expect(persisted).toMatchObject({
      terminalAppearanceVersion: 5,
      terminalThemeMode: "nord",
    });
  });

  it("normalizes, applies, and persists bounded terminal typography", () => {
    expect(parseTerminalFontSize(10)).toBe(10);
    expect(parseTerminalFontSize("28")).toBe(28);
    expect(parseTerminalFontSize(9)).toBe(DEFAULT_TERMINAL_FONT_SIZE);
    expect(parseTerminalFontSize("13.5")).toBe(DEFAULT_TERMINAL_FONT_SIZE);
    expect(normalizeTerminalFontFamily("  JetBrains   Mono ")).toBe("JetBrains Mono");
    expect(DEFAULT_TERMINAL_FONT_FAMILY).toBe("Menlo");
    expect(normalizeTerminalFontFamily("Menlo, monospace")).toBe("Menlo");
    expect(terminalFontCssFamily("JetBrains Mono")).toContain('"JetBrains Mono"');
    expect(terminalFontCssFamily(DEFAULT_TERMINAL_FONT_FAMILY)).toMatch(/^"Menlo",/);
    expect(terminalFontCssFamily("")).toMatch(/^Monaco, SF Mono, SFMono-Regular, Menlo,/);
    expect(parseTerminalFontWeight("600")).toBe(600);
    expect(parseTerminalFontWeight(450)).toBe(400);
    expect(parseTerminalLineHeight("1.6")).toBe(1.6);
    expect(parseTerminalLineHeight(3)).toBe(DEFAULT_TERMINAL_LINE_HEIGHT);
    expect(parseTerminalLetterSpacing("-1")).toBe(-1);
    expect(parseTerminalLetterSpacing(0.5)).toBe(0);
    expect(parseTerminalCursorStyle("bar")).toBe("bar");
    expect(parseTerminalCursorStyle("beam")).toBe("block");

    const store = useUiStore();
    expect(store.terminalFontFamily).toBe("Menlo");
    store.setTerminalFontFamily("JetBrains Mono");
    store.setTerminalFontSize(17);
    store.setTerminalFontWeight(500);
    store.setTerminalBoldFontWeight(800);
    store.setTerminalLineHeight(1.4);
    store.setTerminalLetterSpacing(1);
    store.setTerminalCursorStyle("underline");
    store.setTerminalCursorBlink(false);

    expect(store.terminalFontFamily).toBe("JetBrains Mono");
    expect(store.terminalFontSize).toBe(17);
    expect(JSON.parse(
      localStorage.getItem("norishell.ui.preferences.v1") ?? "{}",
    )).toMatchObject({
      terminalAppearanceVersion: 5,
      terminalFontFamily: "JetBrains Mono",
      terminalFontSize: 17,
      terminalFontWeight: 500,
      terminalBoldFontWeight: 800,
      terminalLineHeight: 1.4,
      terminalLetterSpacing: 1,
      terminalCursorStyle: "underline",
      terminalCursorBlink: false,
    });
  });

  it("applies and persists a custom ANSI palette as a non-secret UI preference", () => {
    const store = useUiStore();
    store.setTerminalThemeMode("custom");
    store.setCustomTerminalColor("background", "#112233");
    store.setCustomTerminalColor("brightCyan", "#44aacc");

    expect(store.resolvedTerminalPalette.background).toBe("#112233");
    expect(document.documentElement.style.getPropertyValue(
      "--nvx-color-terminal-bg",
    )).toBe("#112233");

    const persisted = JSON.parse(
      localStorage.getItem("norishell.ui.preferences.v1") ?? "{}",
    ) as {
      terminalThemeMode?: string;
      customTerminalPalette?: Record<string, string>;
    };
    expect(persisted.terminalThemeMode).toBe("custom");
    expect(persisted.customTerminalPalette?.brightCyan).toBe("#44aacc");
    expect(Object.keys(persisted.customTerminalPalette ?? {})).toHaveLength(
      TERMINAL_COLOR_KEYS.length,
    );
  });

  it("saves one named custom scheme atomically and selects it", () => {
    const store = useUiStore();
    const custom = { ...LIGHT_TERMINAL_PALETTE, background: "#f0f0f0" };

    expect(store.saveCustomTerminalPalette("  Operations  ", custom)).toBe(true);
    expect(store.terminalThemeMode).toBe("custom");
    expect(store.customTerminalPaletteName).toBe("Operations");
    expect(store.hasCustomTerminalPalette).toBe(true);
    expect(store.resolvedTerminalPalette.background).toBe("#f0f0f0");
  });
});
