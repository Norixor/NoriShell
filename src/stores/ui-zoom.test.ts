import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { UI_PREFERENCES_KEY } from "../ui-preferences";
import { useUiStore } from "./ui";

const zoom = vi.hoisted(() => vi.fn<(value: number) => Promise<void>>());
vi.mock("../ui-zoom", async (original) => ({
  ...await original<typeof import("../ui-zoom")>(),
  applyUiZoom: zoom,
}));

describe("interface zoom persistence", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    zoom.mockReset().mockResolvedValue();
    localStorage.clear();
    setActivePinia(createPinia());
  });

  it("restores bounded preferences without changing terminal font configuration", async () => {
    localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify({ uiZoom: 125, terminalFontSize: 17 }));
    const ui = useUiStore();
    expect(ui.uiZoom).toBe(125);
    expect(ui.appliedUiZoom).toBe(100);
    expect(await ui.setUiZoom(ui.uiZoom, false)).toBe(true);
    expect(ui.appliedUiZoom).toBe(125);
    expect(ui.terminalFontSize).toBe(17);
    expect(await ui.setUiZoom(300)).toBe(false);
    expect(await ui.setUiZoom("125")).toBe(false);
    expect(zoom).toHaveBeenCalledTimes(1);
  });

  it("keeps preferences when the native call fails", async () => {
    const ui = useUiStore();
    zoom.mockRejectedValueOnce(new Error("native failure"));
    expect(await ui.setUiZoom(150)).toBe(false);
    expect(ui.uiZoom).toBe(100);
    expect(ui.appliedUiZoom).toBe(100);
    expect(localStorage.getItem(UI_PREFERENCES_KEY)).toBeNull();
  });

  it("rolls the view back when persistence fails", async () => {
    const ui = useUiStore();
    vi.spyOn(localStorage, "setItem").mockImplementationOnce(() => { throw new Error("quota"); });
    expect(await ui.setUiZoom(150)).toBe(false);
    expect(zoom.mock.calls).toEqual([[150], [100]]);
    expect(ui.uiZoom).toBe(100);
    expect(ui.appliedUiZoom).toBe(100);
  });

  it("rejects concurrent zoom requests until the pending native call settles", async () => {
    const ui = useUiStore();
    let complete!: () => void;
    zoom.mockReturnValueOnce(new Promise<void>((resolve) => { complete = resolve; }));
    const first = ui.setUiZoom(125);
    expect(await ui.setUiZoom(150)).toBe(false);
    expect(ui.uiZoomBusy).toBe(true);
    complete();
    expect(await first).toBe(true);
    expect(ui.uiZoomBusy).toBe(false);
    expect(ui.uiZoom).toBe(125);
  });
});
