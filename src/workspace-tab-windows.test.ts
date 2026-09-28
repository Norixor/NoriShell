import { describe, expect, it } from "vitest";

import { workspaceTabViewLabel } from "./workspace-tab-windows";

describe("native Workspace Tab WebView labels", () => {
  it("uses the same Tauri-safe identity as the Rust manager for plugin pages", () => {
    expect(workspaceTabViewLabel("page:plugin:com.norishell.self-host-sync:sync"))
      .toBe("workspace-tab-cGFnZTpwbHVnaW46Y29tLm5vcmlzaGVsbC5zZWxmLWhvc3Qtc3luYzpzeW5j");
  });
});
