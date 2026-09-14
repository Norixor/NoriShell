import { describe, expect, it } from "vitest";

import type { PluginCatalogEntryDto } from "../../core-api/client";
import { comparePluginVersions, latestCompatiblePluginUpdate } from "./pluginCatalogVersion";

const entry = (version: string, compatibility: PluginCatalogEntryDto["compatibility"] = "compatible") => ({
  pluginId: "org.norixor",
  version,
  compatibility,
}) as PluginCatalogEntryDto;

describe("plugin catalog versions", () => {
  it("orders stable and prerelease semantic versions", () => {
    expect(comparePluginVersions("1.0.2", "1.0.1")).toBeGreaterThan(0);
    expect(comparePluginVersions("1.0.2", "1.0.2-rc.1")).toBeGreaterThan(0);
    expect(comparePluginVersions("1.0.2-rc.2", "1.0.2-rc.10")).toBeLessThan(0);
    expect(comparePluginVersions("1.0.2+build.2", "1.0.2+build.1")).toBe(0);
    expect(comparePluginVersions("18446744073709551616.0.0", "18446744073709551615.0.0")).toBe(1);
    expect(comparePluginVersions("1.0.0-alpha", "1.0.0-beta")).toBe(-1);
    expect(comparePluginVersions("latest", "1.0.0")).toBeNull();
  });

  it("selects only the newest compatible version above the installed version", () => {
    expect(latestCompatiblePluginUpdate([
      entry("1.0.1"),
      entry("1.1.0"),
      entry("2.0.0", "appVersionIncompatible"),
      { ...entry("3.0.0"), pluginId: "org.example.other" },
    ], "org.norixor", "1.0.0")?.version).toBe("1.1.0");
    expect(latestCompatiblePluginUpdate([entry("1.0.0")], "org.norixor", "1.0.0")).toBeNull();
  });
});
