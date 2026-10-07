import { describe, expect, it, vi } from "vitest";

import { retryWhileCoreStarting } from "./core-startup-retry";

const notManaged = "state not managed for field `hosts` on command `application_preferences_get`. You must call `.manage()` before using this command";

describe("retryWhileCoreStarting", () => {
  it("retries the Core-not-ready rejection until it succeeds", async () => {
    const run = vi.fn().mockRejectedValueOnce(notManaged).mockRejectedValueOnce(notManaged).mockResolvedValue("ok");
    await expect(retryWhileCoreStarting(run, { enabled: true, sleep: async () => undefined })).resolves.toBe("ok");
    expect(run).toHaveBeenCalledTimes(3);
  });

  it("does not retry other failures or when disabled", async () => {
    const other = vi.fn().mockRejectedValue({ code: "application_preferences.persistence_unavailable" });
    await expect(retryWhileCoreStarting(other, { enabled: true, sleep: async () => undefined })).rejects.toBeTruthy();
    expect(other).toHaveBeenCalledTimes(1);
    const disabled = vi.fn().mockRejectedValue(notManaged);
    await expect(retryWhileCoreStarting(disabled, { enabled: false })).rejects.toBe(notManaged);
    expect(disabled).toHaveBeenCalledTimes(1);
  });

  it("gives up after the bounded wait", async () => {
    const run = vi.fn().mockRejectedValue(notManaged);
    await expect(retryWhileCoreStarting(run, { enabled: true, maxWaitMs: 500, sleep: async () => undefined })).rejects.toBe(notManaged);
  });
});
