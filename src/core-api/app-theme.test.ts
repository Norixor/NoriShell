import { beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

import { listPluginThemes } from "./app-theme";

describe("theme package Core client", () => {
  beforeEach(() => { tauri.invoke.mockReset().mockResolvedValue({ themes: [] }); });

  it("only reads verified installed theme projections from the Core command", async () => {
    await listPluginThemes();
    expect(tauri.invoke).toHaveBeenCalledWith("plugin_theme_list", {
      request: { meta: { requestId: expect.any(String) } },
    });
  });
});
