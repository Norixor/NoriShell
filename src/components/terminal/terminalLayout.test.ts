import { describe, expect, it } from "vitest";

import {
  closeTerminalPane,
  countTerminalPanes,
  createTerminalPane,
  findTerminalPane,
  parseTerminalLayout,
  setTerminalForPane,
  setTerminalSplitRatio,
  splitTerminalPane,
  terminalPaneDepth,
  terminalLayoutDepth,
  type TerminalLayoutNode,
} from "./terminalLayout";

describe("terminalLayout", () => {
  it("creates nested horizontal and vertical splits with a stable active target", () => {
    let layout: TerminalLayoutNode = createTerminalPane("pane-1", "terminal-1");
    layout = splitTerminalPane(layout, "pane-1", "horizontal", "pane-2", "split-1");
    layout = splitTerminalPane(layout, "pane-2", "vertical", "pane-3", "split-2");

    expect(countTerminalPanes(layout)).toBe(3);
    expect(terminalLayoutDepth(layout)).toBe(2);
    expect(findTerminalPane(layout, "pane-1")?.terminalId).toBe("terminal-1");
    expect(findTerminalPane(layout, "pane-3")?.terminalId).toBe("");

    layout = setTerminalForPane(layout, "pane-3", "terminal-3");
    expect(findTerminalPane(layout, "pane-3")?.terminalId).toBe("terminal-3");
  });

  it("collapses the parent split when a pane closes", () => {
    let layout: TerminalLayoutNode = createTerminalPane("pane-1");
    layout = splitTerminalPane(layout, "pane-1", "horizontal", "pane-2", "split-1");
    const closed = closeTerminalPane(layout, "pane-2");

    expect(closed.node).toStrictEqual(createTerminalPane("pane-1"));
    expect(closed.nextActivePaneId).toBe("pane-1");
  });

  it("clamps resize ratios without imposing a Pane-count limit", () => {
    let layout: TerminalLayoutNode = createTerminalPane("pane-1");
    for (let index = 2; index <= 12; index += 1) {
      layout = splitTerminalPane(
        layout,
        `pane-${index - 1}`,
        index % 2 === 0 ? "horizontal" : "vertical",
        `pane-${index}`,
        `split-${index - 1}`,
      );
    }

    expect(countTerminalPanes(layout)).toBe(12);
    expect(terminalLayoutDepth(layout)).toBe(11);
    const resized = setTerminalSplitRatio(layout, "split-1", 0);
    expect(resized.kind === "split" ? resized.ratio : null).toBe(Number.EPSILON);
    expect(terminalPaneDepth(layout, "pane-12")).toBe(11);
  });

  it("rebalances same-axis space when a new Pane fits in the full layout", () => {
    let layout: TerminalLayoutNode = createTerminalPane("pane-1");
    layout = splitTerminalPane(layout, "pane-1", "horizontal", "pane-2", "split-1");
    layout = setTerminalSplitRatio(layout, "split-1", 0.4);
    layout = splitTerminalPane(layout, "pane-2", "horizontal", "pane-3", "split-2");

    expect(layout).toMatchObject({
      kind: "split",
      ratio: 1 / 3,
      second: {
        kind: "split",
        ratio: 0.5,
      },
    });
  });

  it("restores layouts with more than eight Panes", () => {
    let layout: TerminalLayoutNode = createTerminalPane("pane-1");
    for (let index = 2; index <= 9; index += 1) {
      layout = splitTerminalPane(
        layout,
        "pane-1",
        index % 2 === 0 ? "horizontal" : "vertical",
        `pane-${index}`,
        `split-${index - 1}`,
      );
    }

    const restored = parseTerminalLayout(layout);
    expect(restored).not.toBeNull();
    expect(countTerminalPanes(restored!)).toBe(9);
  });

  it("restores a bounded layout without changing pane identities or ratios", () => {
    const restored = parseTerminalLayout({
      kind: "split",
      splitId: "split-1",
      direction: "horizontal",
      ratio: 0.4,
      first: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
      second: { kind: "pane", paneId: "pane-2", terminalId: "" },
    });

    expect(restored).toStrictEqual({
      kind: "split",
      splitId: "split-1",
      direction: "horizontal",
      ratio: 0.4,
      first: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
      second: { kind: "pane", paneId: "pane-2", terminalId: "" },
    });
  });

  it("rejects corrupt, duplicate, unsafe, and over-deep persisted layouts", () => {
    expect(parseTerminalLayout({ kind: "pane", paneId: "", terminalId: "" })).toBeNull();
    expect(parseTerminalLayout({
      kind: "split",
      splitId: "split-1",
      direction: "vertical",
      ratio: 0.5,
      first: { kind: "pane", paneId: "pane-1", terminalId: "" },
      second: { kind: "pane", paneId: "pane-1", terminalId: "" },
    })).toBeNull();
    expect(parseTerminalLayout({
      kind: "split",
      splitId: "split-1",
      direction: "horizontal",
      ratio: 0,
      first: { kind: "pane", paneId: "pane-1", terminalId: "" },
      second: { kind: "pane", paneId: "pane-2", terminalId: "" },
    })).toBeNull();

    let tooDeep: unknown = { kind: "pane", paneId: "pane-66", terminalId: "" };
    for (let index = 65; index >= 1; index -= 1) {
      tooDeep = {
        kind: "split",
        splitId: `split-${index}`,
        direction: index % 2 === 0 ? "vertical" : "horizontal",
        ratio: 0.5,
        first: { kind: "pane", paneId: `pane-${index}`, terminalId: "" },
        second: tooDeep,
      };
    }
    expect(parseTerminalLayout(tooDeep)).toBeNull();
  });
});
