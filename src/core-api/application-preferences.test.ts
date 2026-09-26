import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke, isTauri: () => true }));

import { applicationPreferenceFailure, installApplicationPreferencesSnapshot, saveApplicationPreferences } from "./application-preferences";

beforeEach(() => mocks.invoke.mockReset());

describe("Core-owned application preferences", () => {
  it("preserves each Core failure code for the UI", async () => {
    installApplicationPreferencesSnapshot({ group: "files", revision: "1", value: { browser: { sort: "name" } } });
    for (const [code, kind] of [
      ["application_preferences.conflict", "conflict"],
      ["application_preferences.invalid_input", "invalidInput"],
      ["application_preferences.persistence_unavailable", "persistenceUnavailable"],
      ["application_preferences.unsupported_schema", "unsupportedSchema"],
      ["application_preferences.requires_reconciliation", "requiresReconciliation"],
    ] as const) {
      const error = { code, messageKey: "errors.applicationPreferences", requestId: "request" };
      mocks.invoke.mockRejectedValueOnce(error);
      await expect(saveApplicationPreferences("files", { browser: { sort: "size" } }, { browser: { sort: "name" } })).rejects.toBe(error);
      expect(applicationPreferenceFailure(error)).toBe(kind);
    }
  });

  it("treats a stale local snapshot and a malformed success response as actionable failures", async () => {
    installApplicationPreferencesSnapshot({ group: "files", revision: "2", value: { browser: { sort: "name" } } });
    await expect(saveApplicationPreferences("files", { browser: { sort: "size" } }, { browser: { sort: "modified" } }))
      .rejects.toMatchObject({ code: "application_preferences.conflict" });
    expect(mocks.invoke).not.toHaveBeenCalled();

    mocks.invoke.mockResolvedValueOnce({ group: "files", revision: "3", value: { browser: { sort: "modified" } } });
    await expect(saveApplicationPreferences("files", { browser: { sort: "size" } }, { browser: { sort: "name" } }))
      .rejects.toMatchObject({ code: "application_preferences.requires_reconciliation" });
  });
});
