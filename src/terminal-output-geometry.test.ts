import { describe, expect, it } from "vitest";
import { Terminal } from "@xterm/xterm";

import { geometryForOutput, recordOutputGeometry } from "./terminal-output-geometry";

describe("terminal output geometry across repeated handoffs", () => {
  it("replays startup output at its original width after a narrower window moves back", () => {
    const markers = [{ afterOutputSeq: "0", rows: 30, cols: 132 }];
    expect(geometryForOutput(markers, "1")?.cols).toBe(132);

    // The child window resizes after the startup frame was rendered.
    recordOutputGeometry(markers, "1", 30, 80);
    expect(geometryForOutput(markers, "1")?.cols).toBe(132);
    expect(geometryForOutput(markers, "2")?.cols).toBe(80);

    // Moving back replays the same Core ring, then adapts future output to the main window.
    recordOutputGeometry(markers, "2", 30, 132);
    expect(geometryForOutput(markers, "1")?.cols).toBe(132);
    expect(geometryForOutput(markers, "2")?.cols).toBe(80);
    expect(geometryForOutput(markers, "3")?.cols).toBe(132);
  });

  it("coalesces resizes without intervening output", () => {
    const markers = [{ afterOutputSeq: "0", rows: 24, cols: 80 }];
    recordOutputGeometry(markers, "0", 24, 100);
    recordOutputGeometry(markers, "0", 24, 120);
    expect(markers).toEqual([{ afterOutputSeq: "0", rows: 24, cols: 120 }]);
  });

  it("keeps zsh's startup percent erase on the original line", async () => {
    const startup = new TextEncoder().encode(`\u001b[7m%\u001b[27m${" ".repeat(131)}\r \r`);
    const chronological = new Terminal({ cols: 132, rows: 24 });
    const wrongWidth = new Terminal({ cols: 80, rows: 24 });
    await Promise.all([
      new Promise<void>((resolve) => chronological.write(startup, resolve)),
      new Promise<void>((resolve) => wrongWidth.write(startup, resolve)),
    ]);
    expect(chronological.buffer.active.getLine(0)?.translateToString().startsWith("% ")).toBe(false);
    expect(wrongWidth.buffer.active.getLine(0)?.translateToString().startsWith("% ")).toBe(true);
    chronological.resize(80, 24);
    expect(chronological.buffer.active.getLine(0)?.translateToString().startsWith("% ")).toBe(false);
    chronological.dispose();
    wrongWidth.dispose();
  });
});
