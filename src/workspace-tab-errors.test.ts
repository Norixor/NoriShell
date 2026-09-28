import { describe, expect, it } from "vitest";

import { workspaceTabFailureCode, workspaceTabFailureMessageKey } from "./workspace-tab-errors";

describe("Workspace Tab failure presentation", () => {
  it("keeps machine codes but excludes arbitrary exception text from diagnostics", () => {
    expect(workspaceTabFailureCode(new Error("workspace_tab.close_failed"))).toBe("workspace_tab.close_failed");
    expect(workspaceTabFailureCode(new Error("request failed: secret payload"))).toBe("workspace_tab.bootstrap_failed");
    expect(workspaceTabFailureCode(new Error("private.token"))).toBe("workspace_tab.bootstrap_failed");
    expect(workspaceTabFailureCode("<script>payload</script>", "workspace_tab.action_failed")).toBe("workspace_tab.action_failed");
    expect(workspaceTabFailureCode(`service.${"a".repeat(121)}`)).toBe("workspace_tab.bootstrap_failed");
  });

  it.each([
    ["workspace_tab.local_terminate_failed", "close"],
    ["workspace_tab.file_disconnect_failed", "close"],
    ["workspace_tab.layout_not_durable", "projection"],
    ["workspace_tab.listener_unavailable", "bootstrap"],
    ["workspace_tab.orphan_resource_unknown", "bootstrap"],
    ["workspace_tab.activation_failed", "action"],
    ["workspace_tab.drag_listener_unavailable", "action"],
  ])("gives %s an actionable message category", (code, category) => {
    expect(workspaceTabFailureMessageKey(code)).toBe(`workspaceTabError.${category}`);
  });
});
