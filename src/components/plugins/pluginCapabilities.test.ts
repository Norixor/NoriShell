import { describe, expect, it } from "vitest";

import { isSpecialPluginCapability } from "./pluginCapabilities";

describe("plugin capability approval grouping", () => {
  it("requires protected approval for independent local data reads", () => {
    expect(isSpecialPluginCapability("appPreferencesRead")).toBe(true);
    expect(isSpecialPluginCapability("terminalHistoryRead")).toBe(true);
  });
});
