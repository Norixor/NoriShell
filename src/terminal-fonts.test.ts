import { describe, expect, it } from "vitest";

import {
  detectInstalledTerminalFonts,
  TERMINAL_FONT_CANDIDATES,
} from "./terminal-fonts";

describe("terminal font discovery", () => {
  it("keeps only installed fonts in the stable terminal-safe order", async () => {
    const installed = new Set(["Menlo", "JetBrains Mono", "Consolas"]);
    const result = await detectInstalledTerminalFonts(async (fontFamily) =>
      installed.has(fontFamily));

    expect(result).toEqual(
      TERMINAL_FONT_CANDIDATES.filter((fontFamily) => installed.has(fontFamily)),
    );
  });

  it("falls back to an empty detected list when no candidate is installed", async () => {
    await expect(detectInstalledTerminalFonts(async () => false)).resolves.toEqual([]);
  });
});
