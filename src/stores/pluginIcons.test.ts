import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

const readPluginIcon = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("../core-api/client", () => ({ readPluginIcon }));

import { usePluginIconsStore } from "./pluginIcons";

beforeEach(() => {
  setActivePinia(createPinia());
  readPluginIcon.mockReset();
});

describe("plugin icon projection", () => {
  it("drops failed artwork only in its scope and retries on the next read", async () => {
    readPluginIcon.mockResolvedValue({ dataUrl: "data:image/png;base64,art" });
    const icons = usePluginIconsStore();
    await icons.load("org.example", "catalog", "1");
    await icons.load("org.example", "installed", "hash");
    icons.discardImage("org.example", "installed", "data:image/png;base64,art");
    expect(icons.imageFor("org.example", "installed")).toBeUndefined();
    expect(icons.imageFor("org.example", "catalog")).toContain("art");
    await icons.load("org.example", "installed", "hash");
    expect(readPluginIcon).toHaveBeenCalledTimes(3);
    expect(icons.imageFor("org.example", "installed")).toContain("art");
  });

  it("ignores a late image error after artwork has been refreshed", async () => {
    readPluginIcon.mockResolvedValueOnce({ dataUrl: "data:image/png;base64,old" });
    readPluginIcon.mockResolvedValueOnce({ dataUrl: "data:image/png;base64,new" });
    const icons = usePluginIconsStore();
    await icons.load("org.example", "catalog", "1");
    await icons.load("org.example", "catalog", "1", true);
    icons.discardImage("org.example", "catalog", "data:image/png;base64,old");
    expect(icons.imageFor("org.example", "catalog")).toContain("new");
  });

  it("keeps catalog artwork separate from an unverified local installation", async () => {
    readPluginIcon.mockResolvedValueOnce({ dataUrl: "data:image/png;base64,official" });
    readPluginIcon.mockResolvedValueOnce({ dataUrl: null });
    const icons = usePluginIconsStore();
    await icons.load("org.example", "catalog", "1");
    await icons.load("org.example", "installed", "local-hash");
    expect(icons.imageFor("org.example", "catalog")).toContain("official");
    expect(icons.imageFor("org.example", "installed")).toBeUndefined();
  });

  it("drops an old package response after installation provenance changes", async () => {
    let finishOld!: (result: { dataUrl: string }) => void;
    readPluginIcon.mockImplementationOnce(() => new Promise((resolve) => { finishOld = resolve; }));
    readPluginIcon.mockResolvedValueOnce({ dataUrl: null });
    const icons = usePluginIconsStore();
    const old = icons.load("org.example", "installed", "official-hash");
    await icons.load("org.example", "installed", "local-hash");
    finishOld({ dataUrl: "data:image/png;base64,old" });
    await old;
    expect(icons.imageFor("org.example", "installed")).toBeUndefined();
  });

  it("deduplicates reads and makes explicit refresh bypass the current projection", async () => {
    readPluginIcon.mockResolvedValue({ dataUrl: "data:image/png;base64,new" });
    const icons = usePluginIconsStore();
    await Promise.all([icons.load("org.example", "catalog", "1"), icons.load("org.example", "catalog", "1")]);
    expect(readPluginIcon).toHaveBeenCalledTimes(1);
    await icons.load("org.example", "catalog", "1", true);
    expect(readPluginIcon).toHaveBeenLastCalledWith("org.example", "catalog", true);
    expect(readPluginIcon).toHaveBeenCalledTimes(2);
  });

  it("does not restore an uninstalled plugin from an in-flight response", async () => {
    let finish!: (result: { dataUrl: string }) => void;
    readPluginIcon.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const icons = usePluginIconsStore();
    const task = icons.load("org.example", "installed", "hash");
    icons.retain([], "installed");
    finish({ dataUrl: "data:image/png;base64,old" });
    await task;
    expect(icons.imageFor("org.example", "installed")).toBeUndefined();
  });
});
