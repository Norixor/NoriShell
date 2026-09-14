import { beforeEach, describe, expect, it, vi } from "vitest";

const core = vi.hoisted(() => ({
  invoke: vi.fn(),
  isTauri: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => core);

import { exportJsonFile } from "./platform-file-export";

describe("exportJsonFile", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    core.isTauri.mockReturnValue(false);
  });

  it("uses the Core-owned JSON exporter in Tauri and preserves a cancelled result", async () => {
    core.isTauri.mockReturnValue(true);
    core.invoke.mockResolvedValue(false);

    await expect(exportJsonFile("preferences", "{}")).resolves.toBe(false);
    expect(core.invoke).toHaveBeenCalledWith("native_json_export", { kind: "preferences", text: "{}" });
  });

  it("keeps Blob downloads for browser previews", async () => {
    const createObjectURL = vi.fn().mockReturnValue("blob:norishell");
    const revokeObjectURL = vi.fn();
    const click = vi.fn();
    vi.stubGlobal("URL", { createObjectURL, revokeObjectURL });
    vi.spyOn(document, "createElement").mockReturnValue({ href: "", download: "", click } as unknown as HTMLAnchorElement);

    await expect(exportJsonFile("shortcuts", "{}")) .resolves.toBe(true);
    expect(createObjectURL).toHaveBeenCalledOnce();
    expect(click).toHaveBeenCalledOnce();
    expect(core.invoke).not.toHaveBeenCalled();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });
});
