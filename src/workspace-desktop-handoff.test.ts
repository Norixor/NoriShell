import { describe, expect, it } from "vitest";

import { filterDesktopSessions, parseDesktopHandoff } from "./workspace-desktop-handoff";
import type { WorkspaceTabSnapshot } from "./workspace-tab-windows";

const sessions = [{ id: "main" }, { id: "child" }];
const sharedWindow: WorkspaceTabSnapshot = {
  owned: ["main", "child"].map((id) => ({ id: `desktop:${id}`, kind: "desktop" as const, owner: "main", payload: {} })),
  others: [],
};

describe("desktop Tab recovery", () => {
  it("only transports the exact live session handle", () => {
    const payload = { schemaVersion: 1, tabId: "desktop:main", sessionId: "main", generation: "42" };
    expect(parseDesktopHandoff(payload, "desktop:main")).toEqual(payload);
    expect(parseDesktopHandoff({ ...payload, sessionId: "replacement" }, "desktop:main"))
      .toEqual({ ...payload, sessionId: "replacement" });
    expect(parseDesktopHandoff({ schemaVersion: 1, tabId: "desktop:main", profileId: "saved-profile" }, "desktop:main"))
      .toEqual({ schemaVersion: 1, tabId: "desktop:main", profileId: "saved-profile" });
    expect(parseDesktopHandoff({ ...payload, password: "secret" }, "desktop:main")).toBeNull();
    expect(parseDesktopHandoff({ ...payload, generation: "43" }, "desktop:other")).toBeNull();
    expect(parseDesktopHandoff({ ...payload, generation: "wrong" }, "desktop:main")).toBeNull();
  });

  it("never renders a session in a shell page", () => {
    expect(filterDesktopSessions(sessions, sharedWindow, new Set(), null, "main")).toEqual([]);
  });

  it("projects a native desktop session only in its own tab WebView", () => {
    expect(filterDesktopSessions(sessions, sharedWindow, new Set(), "desktop:main", "main").map((item) => item.id))
      .toEqual(["main"]);
    expect(filterDesktopSessions(sessions, sharedWindow, new Set(), "desktop:child", "child").map((item) => item.id))
      .toEqual(["child"]);
    expect(filterDesktopSessions(sessions, sharedWindow, new Set(), "desktop:main", null)).toEqual([]);
    expect(filterDesktopSessions([{ id: "replacement" }], sharedWindow, new Set(), "desktop:main", "replacement")
      .map((item) => item.id)).toEqual(["replacement"]);
    expect(filterDesktopSessions(sessions, { owned: [], others: [] }, new Set(), "desktop:main", "main")).toEqual([]);
    expect(filterDesktopSessions(sessions, { owned: [], others: [] }, new Set(["desktop:main"]), "desktop:main", "main")
      .map((item) => item.id)).toEqual(["main"]);
  });
});
