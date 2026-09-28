import { beforeEach, describe, expect, it } from "vitest";

import {
  replaceWorkspaceTabViewSummary,
  retainWorkspaceTabViewSummaries,
  workspaceTabViewFallbackAfterRemoval,
  setWorkspaceTabViewSummary,
  workspaceTabViewSummaries,
} from "./workspace-tab-view-state";

describe("native Workspace Tab ordering", () => {
  beforeEach(() => retainWorkspaceTabViewSummaries(new Set()));

  it("puts a created target in its New Page source's place without an intermediate Tab", () => {
    for (const id of ["terminal:existing", "page:newPage", "file:other"]) {
      setWorkspaceTabViewSummary({ id, viewLabel: id, kind: id.startsWith("page:") ? "page" : "terminal",
        label: id, stateLabel: "" });
    }
    replaceWorkspaceTabViewSummary("page:newPage", { id: "terminal:new", viewLabel: "terminal:new",
      kind: "terminal", label: "new", stateLabel: "" });
    expect(workspaceTabViewSummaries.value.map((tab) => tab.id)).toEqual([
      "terminal:existing", "terminal:new", "file:other",
    ]);
  });

  it("selects the nearest surviving Tab when the active native view closes", () => {
    for (const id of ["page:first", "page:second", "page:third"]) {
      setWorkspaceTabViewSummary({ id, viewLabel: id, kind: "page", label: id, stateLabel: "" });
    }
    expect(workspaceTabViewFallbackAfterRemoval("page:second", new Set(["page:first", "page:third"])))
      .toBe("page:first");
    expect(workspaceTabViewFallbackAfterRemoval("page:first", new Set(["page:third"])))
      .toBe("page:third");
    expect(workspaceTabViewFallbackAfterRemoval("page:first", new Set())).toBeNull();
  });
});
