import { describe, expect, it, vi } from "vitest";

import { applyPreferencePreview, exportPreferenceTransfer, parsePreferenceTransfer, previewPreferenceTransfer, type PreferenceGroupAdapter } from "./preferences-transfer";

function fixture() {
  let current = { enabled: false };
  const adapter: PreferenceGroupAdapter = {
    id: "highlights",
    validate: (value) => value !== null && typeof value === "object" && Object.keys(value).length === 1 && "enabled" in value && typeof value.enabled === "boolean",
    read: async () => current,
    apply: vi.fn(async (value) => { current = value as typeof current; return true; }),
    defaults: () => ({ enabled: false }),
  };
  return { adapter, change: () => { current = { enabled: true }; } };
}

describe("preference file protocol and immutable preview", () => {
  it("rejects unknown groups, fields and oversized input before applying anything", () => {
    const { adapter } = fixture();
    const file = { product: "NoriShell", version: 1, groups: { vault: { password: "test" } } };
    expect(() => parsePreferenceTransfer(JSON.stringify(file), [adapter])).toThrow("invalidGroup");
    expect(() => parsePreferenceTransfer(JSON.stringify({ ...file, credentials: {} }), [adapter])).toThrow("invalidFile");
    expect(() => parsePreferenceTransfer(" ".repeat(256 * 1024 + 1), [adapter])).toThrow("tooLarge");
    expect(adapter.apply).not.toHaveBeenCalled();
  });

  it("exports only explicitly selected adapters and validates the result", async () => {
    const { adapter } = fixture();
    const text = await exportPreferenceTransfer([adapter]);
    expect(parsePreferenceTransfer(text, [adapter]).groups).toEqual({ highlights: { enabled: false } });
    await expect(exportPreferenceTransfer([])).rejects.toThrow("emptySelection");
  });

  it("freezes the preview payload and does not replay a completed group", async () => {
    const { adapter } = fixture();
    const value = { enabled: true };
    const preview = await previewPreferenceTransfer({ product: "NoriShell", version: 1, groups: { highlights: value } }, [adapter]);
    value.enabled = false;
    expect(() => Object.assign(preview[0]!, { after: "{}" })).toThrow();
    const selected = new Set([adapter.id]);
    await Promise.all([applyPreferencePreview(preview, selected, [adapter]), applyPreferencePreview(preview, selected, [adapter])]);
    await applyPreferencePreview(preview, selected, [adapter]);
    expect(adapter.apply).toHaveBeenCalledExactlyOnceWith({ enabled: true }, { enabled: false });
    expect(preview[0]?.result).toBe("applied");
  });

  it("requires a new preview after drift or failure instead of replaying", async () => {
    const { adapter, change } = fixture();
    const preview = await previewPreferenceTransfer({ product: "NoriShell", version: 1, groups: { highlights: { enabled: true } } }, [adapter]);
    change();
    await applyPreferencePreview(preview, new Set([adapter.id]), [adapter]);
    expect(preview[0]?.result).toBe("conflict");
    expect(adapter.apply).not.toHaveBeenCalled();
  });
});
