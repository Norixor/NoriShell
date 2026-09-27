import { beforeEach, describe, expect, it, vi } from "vitest";

const ownership = vi.hoisted(() => ({
  snapshot: vi.fn(),
  focus: vi.fn(),
  label: "main",
}));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("./workspace-tab-windows", () => ({
  snapshotWorkspaceTabs: ownership.snapshot,
  focusWorkspaceWindowTarget: ownership.focus,
  workspaceWindowLabel: () => ownership.label,
}));

import { guardPluginPageRoute } from "./router";

const id = "page:plugin:org.example.sync:home";
const tab = { id, kind: "page", owner: "main", revision: 1, payload: {} };

describe("plugin Page Tab route ownership", () => {
  beforeEach(() => {
    ownership.label = "main";
    ownership.focus.mockReset().mockResolvedValue(undefined);
    ownership.snapshot.mockReset().mockResolvedValue({ owned: [], incoming: [], outgoing: [], others: [] });
  });

  it("keeps the source window out of a plugin page during handoff", async () => {
    ownership.snapshot.mockResolvedValue({
      owned: [tab], incoming: [],
      outgoing: [{ ticket: "move", tab, source: "main", target: "workspace-a", phase: "offered" }],
      others: [],
    });
    expect(await guardPluginPageRoute("/plugin/org.example.sync/home")).toBe("/terminal");
  });

  it("keeps a receiving window on its fallback until Core commits ownership", async () => {
    ownership.label = "workspace-a";
    ownership.snapshot.mockResolvedValue({
      owned: [],
      incoming: [{ ticket: "move", tab, source: "main", target: "workspace-a", phase: "offered" }],
      outgoing: [], others: [{ id, kind: "page", owner: "main", terminalPanes: [] }],
    });
    expect(await guardPluginPageRoute("/plugin/org.example.sync/home")).toBe("/workspace-window");
    expect(ownership.focus).not.toHaveBeenCalled();
  });

  it("focuses the committed owner instead of opening a second view", async () => {
    ownership.snapshot.mockResolvedValue({
      owned: [], incoming: [], outgoing: [],
      others: [{ id, kind: "page", owner: "workspace-a", terminalPanes: [] }],
    });
    expect(await guardPluginPageRoute("/plugin/org.example.sync/home")).toBe("/terminal");
    expect(ownership.focus).toHaveBeenCalledWith("workspace-a");
  });

  it("preserves ownership lookup failure instead of allowing an unverified page", async () => {
    ownership.snapshot.mockRejectedValue("workspace_tab.unavailable");
    await expect(guardPluginPageRoute("/plugin/org.example.sync/home"))
      .rejects.toBe("workspace_tab.unavailable");
  });
});
