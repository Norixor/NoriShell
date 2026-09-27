import { describe, expect, it } from "vitest";

import type { PersistedTerminalWorkspaceLayout } from "./components/terminal/terminalWorkspaceLayout";
import {
  MAX_TERMINAL_TAB_HANDOFF_BYTES,
  mergeOwnedTerminalWorkspaceLayout,
  parseTerminalTabHandoff,
} from "./terminal-workspace-handoff";

function tab(tabId: string) {
  const paneId = `${tabId}-pane`;
  return {
    tabId, activePaneId: paneId,
    layout: { kind: "pane" as const, paneId, terminalId: paneId },
    panes: [{ kind: "launcher" as const, paneId, label: "New terminal" }],
  };
}

function layout(tabIds: string[], activeTabId: string | null): PersistedTerminalWorkspaceLayout {
  return { schemaVersion: 1, activeTabId, tabs: tabIds.map(tab) };
}

describe("terminal tab handoff payload", () => {
  it("accepts a live Session reference without copying a credential", () => {
    const input = { schemaVersion: 1, tabId: "tab-1", activePaneId: "pane-1",
      layout: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
      panes: [{ kind: "ssh", paneId: "pane-1", label: "Host", sessionId: "session-1", generation: "3",
        initialDimensions: { rows: 38, cols: 132 },
        outputGeometry: [{ afterOutputSeq: "0", rows: 38, cols: 132 },
          { afterOutputSeq: "17", rows: 38, cols: 80 }] }] };
    expect(parseTerminalTabHandoff(input)).toEqual(input);
    expect(parseTerminalTabHandoff({ ...input, credentialRefId: "credential-1" })).toBeNull();
    expect(parseTerminalTabHandoff({ ...input, panes: [{ ...input.panes[0], token: "secret" }] })).toBeNull();
    expect(parseTerminalTabHandoff({ ...input, panes: [{ ...input.panes[0],
      initialDimensions: { rows: 38, cols: 0 } }] })).toBeNull();
    expect(parseTerminalTabHandoff({ ...input, panes: [{ ...input.panes[0],
      outputGeometry: [{ afterOutputSeq: "17", rows: 38, cols: 80 }] }] })).toBeNull();
    expect(parseTerminalTabHandoff({ ...input, panes: [{ ...input.panes[0],
      outputGeometry: [...input.panes[0]!.outputGeometry,
        { afterOutputSeq: "15", rows: 38, cols: 100 }] }] })).toBeNull();
    expect(parseTerminalTabHandoff({ ...input, panes: [{ ...input.panes[0],
      outputGeometry: [{ afterOutputSeq: "0", rows: 38, cols: 132 },
        ...Array.from({ length: 4096 }, (_, index) => ({ afterOutputSeq: String(index + 1), rows: 38, cols: 80 }))] }] })).toBeNull();
  });

  it("rejects a mismatched split tree and oversized IPC data", () => {
    const input = { schemaVersion: 1, tabId: "tab-1", activePaneId: "pane-1",
      layout: { kind: "pane", paneId: "pane-2", terminalId: "pane-2" },
      panes: [{ kind: "launcher", paneId: "pane-1", label: "New terminal" }] };
    expect(parseTerminalTabHandoff(input)).toBeNull();
    expect(parseTerminalTabHandoff({ ...input, layout: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
      panes: [{ ...input.panes[0], label: "x".repeat(MAX_TERMINAL_TAB_HANDOFF_BYTES) }] })).toBeNull();
  });

  it("accepts deferred panes without carrying a plugin configuration or secret", () => {
    const input = { schemaVersion: 1, tabId: "tab-1", activePaneId: "pane-1",
      layout: { kind: "pane", paneId: "pane-1", terminalId: "pane-1" },
      panes: [{ kind: "pluginDeferred", paneId: "pane-1", label: "Tool",
        pluginId: "plugin-1", providerId: "provider-1", schemaHash: "hash-1" }] };
    expect(parseTerminalTabHandoff(input)).toEqual(input);
    expect(parseTerminalTabHandoff({ ...input, panes: [{ ...input.panes[0], configuration: { password: "secret" } }] })).toBeNull();
  });
});

describe("owned Terminal layout CAS merge", () => {
  it("preserves another window's Tab while replacing and closing owned Tabs", () => {
    const latest = layout(["main-1", "child-1", "main-2"], "child-1");
    const local = layout(["main-1"], "main-1");
    const merged = mergeOwnedTerminalWorkspaceLayout(latest, local, new Set(["main-1", "main-2"]));
    expect(merged.tabs.map((item) => item.tabId)).toEqual(["main-1", "child-1"]);
    expect(merged.activeTabId).toBe("child-1");
  });

  it("keeps a transferred Tab when the former owner relinquishes it", () => {
    const latest = layout(["main-1", "moved"], "moved");
    const local = layout(["main-1"], "main-1");
    const merged = mergeOwnedTerminalWorkspaceLayout(latest, local, new Set(["main-1"]));
    expect(merged.tabs.map((item) => item.tabId)).toEqual(["main-1", "moved"]);
  });
});
