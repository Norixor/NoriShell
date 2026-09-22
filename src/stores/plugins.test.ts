import { describe, expect, it } from "vitest";

import {
  normalizePluginContributionNodes,
  pluginFailureCode,
  samePluginContributionFence,
} from "./plugins";

describe("declarative plugin contribution renderer", () => {
  it("accepts only host-owned text, status, action and copy nodes", () => {
    expect(normalizePluginContributionNodes([
      { kind: "text", text: "Bounded status" },
      { kind: "status", label: "Ready", tone: "success" },
      { kind: "action", actionId: "generateUuid", label: "Generate UUID" },
      { kind: "copy", copyId: "copyUuid", label: "Copy UUID" },
    ])).toEqual([
      { kind: "text", text: "Bounded status" },
      { kind: "status", label: "Ready", tone: "success" },
      { kind: "action", actionId: "generateUuid", label: "Generate UUID" },
      { kind: "copy", copyId: "copyUuid", label: "Copy UUID" },
    ]);
  });

  it("fails closed for HTML, CSS, SVG, iframe, URL and unknown fields", () => {
    expect(normalizePluginContributionNodes([
      { kind: "html", html: "<img src=x onerror=alert(1)>" },
      { kind: "text", text: "looks safe", url: "https://evil.example" },
      { kind: "svg", path: "M0 0" },
      { kind: "iframe", src: "https://evil.example" },
      { kind: "style", css: "body { display: none }" },
      { kind: "action", actionId: "javascript:alert(1)", label: "Run" },
      { kind: "text", text: "" },
      { kind: "text", text: "unsafe\u0000text" },
    ])).toEqual([]);
  });

  it("rejects a late contribution response from an older runtime generation", () => {
    const panel = {
      pluginId: "com.norishell.utility-demo",
      pluginName: "Utility Demo",
      artifactFingerprintSha256: "a".repeat(64),
      packageSha256: "b".repeat(64),
      instanceGeneration: "4",
      stateVersion: "7",
      contributionRevision: "2",
      slot: "terminalToolbar" as const,
      nodes: [{ kind: "text" as const, text: "current" }],
    };
    expect(samePluginContributionFence(panel, { ...panel, instanceGeneration: "3" })).toBe(false);
    expect(samePluginContributionFence(panel, { ...panel, contributionRevision: "1" })).toBe(false);
    expect(samePluginContributionFence(panel, { ...panel })).toBe(true);
    expect(samePluginContributionFence(panel, { ...panel, slot: "pluginsPage" })).toBe(false);
  });

  it("preserves typed Core plugin failures instead of replacing them with requestFailed", () => {
    expect(pluginFailureCode({ errorCode: "packageHashMismatch" })).toBe("packageHashMismatch");
    expect(pluginFailureCode({ code: "plugin.conflict" })).toBe("installConflict");
    expect(pluginFailureCode({ code: "plugin.invalid_request" })).toBe("invalidRequest");
  });
});
