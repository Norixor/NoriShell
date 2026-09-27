import { describe, expect, it } from "vitest";

import { filterDesktopSessions, parseDesktopHandoff } from "./workspace-desktop-handoff";
import type { WorkspaceTabSnapshot } from "./workspace-tab-windows";

const sessions = [{ id: "main" }, { id: "child" }, { id: "legacy" }];
const state: WorkspaceTabSnapshot = {
  owned: [{ id: "desktop:main", kind: "desktop", owner: "main", revision: 1, payload: {} }],
  incoming: [], outgoing: [],
  others: [{ id: "desktop:child", kind: "desktop", owner: "workspace-1", terminalPanes: [] }],
};

describe("desktop workspace handoff", () => {
  it("only transports the exact live session handle", () => {
    const payload = { schemaVersion: 1, tabId: "desktop:main", sessionId: "main", generation: "42" };
    expect(parseDesktopHandoff(payload, "desktop:main")).toEqual(payload);
    expect(parseDesktopHandoff({ ...payload, password: "secret" }, "desktop:main")).toBeNull();
    expect(parseDesktopHandoff({ ...payload, generation: "43" }, "desktop:other")).toBeNull();
    expect(parseDesktopHandoff({ ...payload, generation: "wrong" }, "desktop:main")).toBeNull();
  });

  it("hides another window's session and restores unregistered sessions to main after child loss", () => {
    expect(filterDesktopSessions(sessions, state, "main").map((item) => item.id)).toEqual(["main", "legacy"]);
    expect(filterDesktopSessions(sessions, { ...state, others: [] }, "main").map((item) => item.id))
      .toEqual(["main", "child", "legacy"]);
  });

  it("limits children to owned, staged, or newly created sessions", () => {
    const childState: WorkspaceTabSnapshot = {
      owned: [{ id: "desktop:child", kind: "desktop", owner: "workspace-1", revision: 2, payload: {} }],
      incoming: [], outgoing: [],
      others: [{ id: "desktop:main", kind: "desktop", owner: "main", terminalPanes: [] }],
    };
    expect(filterDesktopSessions(sessions, childState, "workspace-1").map((item) => item.id)).toEqual(["child"]);
    expect(filterDesktopSessions(sessions, childState, "workspace-1", new Set(["desktop:legacy"])).map((item) => item.id))
      .toEqual(["child", "legacy"]);
    expect(filterDesktopSessions(sessions, childState, "workspace-1", new Set(["desktop:main"])).map((item) => item.id))
      .toEqual(["main", "child"]);
  });
});
