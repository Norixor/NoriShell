import { afterEach, describe, expect, it, vi } from "vitest";

import {
  initializeStartupVaultTip,
  STARTUP_VAULT_TIP_SCOPE,
  type StartupVaultTipStore,
} from "./startup-vault-tip";
import type { ShowNvxTipInput } from "./stores/tips";

function createTips() {
  const items: ShowNvxTipInput[] = [];
  const tips: StartupVaultTipStore = {
    show: vi.fn((input: ShowNvxTipInput) => {
      const existing = items.findIndex((item) => item.scope === input.scope);
      if (existing >= 0) items.splice(existing, 1);
      items.unshift(input);
      return `tip-${items.length}`;
    }),
    dismissScope: vi.fn((scope: string) => {
      for (let index = items.length - 1; index >= 0; index -= 1) {
        if (items[index]?.scope === scope) items.splice(index, 1);
      }
    }),
  };
  return { tips, items };
}

async function flush() {
  await Promise.resolve();
  await Promise.resolve();
}

describe("initializeStartupVaultTip", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each(["locked", "requiresReload"] as const)("shows a ten-second reminder for %s", async (state) => {
    const { tips, items } = createTips();
    const cleanup = initializeStartupVaultTip({
      tips,
      t: (key) => key,
      fetchStatus: vi.fn().mockResolvedValue({ state }),
      eventTarget: new EventTarget(),
    });
    await flush();

    expect(items).toEqual([expect.objectContaining({
      scope: STARTUP_VAULT_TIP_SCOPE,
      tone: "warning",
      durationMs: 10_000,
      title: "tips.startupVault.title",
      message: "tips.startupVault.description",
      action: expect.objectContaining({ label: "tips.startupVault.unlockNow" }),
    })]);
    cleanup();
  });

  it.each(["missing", "unlocked"] as const)("never shows a reminder for %s", async (state) => {
    const { tips, items } = createTips();
    const cleanup = initializeStartupVaultTip({
      tips,
      t: (key) => key,
      fetchStatus: vi.fn().mockResolvedValue({ state }),
      eventTarget: new EventTarget(),
    });
    await flush();

    expect(items).toEqual([]);
    expect(tips.dismissScope).toHaveBeenCalledWith(STARTUP_VAULT_TIP_SCOPE);
    cleanup();
  });

  it("rechecks before opening the secure operation and serializes repeated actions", async () => {
    const { tips, items } = createTips();
    const fetchStatus = vi.fn()
      .mockResolvedValueOnce({ state: "locked" })
      .mockResolvedValueOnce({ state: "locked" })
      .mockResolvedValueOnce({ state: "unlocked" });
    let resolveUnlock: ((value: boolean) => void) | undefined;
    const requestUnlock = vi.fn(() => new Promise<boolean>((resolve) => { resolveUnlock = resolve; }));
    const cleanup = initializeStartupVaultTip({
      tips,
      t: (key) => key,
      fetchStatus,
      requestUnlock,
      eventTarget: new EventTarget(),
    });
    await flush();

    const action = items[0]?.action;
    expect(action).toBeDefined();
    const first = action?.onClick();
    const duplicate = action?.onClick();
    await flush();
    expect(requestUnlock).toHaveBeenCalledOnce();

    resolveUnlock?.(true);
    await Promise.all([first, duplicate]);
    expect(fetchStatus).toHaveBeenCalledTimes(3);
    expect(tips.dismissScope).toHaveBeenCalledWith(STARTUP_VAULT_TIP_SCOPE);
    cleanup();
  });

  it("does not open a secure Vault window when the fresh state is already unlocked", async () => {
    const { tips, items } = createTips();
    const fetchStatus = vi.fn()
      .mockResolvedValueOnce({ state: "locked" })
      .mockResolvedValueOnce({ state: "unlocked" })
      .mockResolvedValueOnce({ state: "unlocked" });
    const requestUnlock = vi.fn().mockResolvedValue(true);
    const cleanup = initializeStartupVaultTip({
      tips,
      t: (key) => key,
      fetchStatus,
      requestUnlock,
      eventTarget: new EventTarget(),
    });
    await flush();

    await items[0]?.action?.onClick();

    expect(requestUnlock).not.toHaveBeenCalled();
    expect(tips.dismissScope).toHaveBeenCalledWith(STARTUP_VAULT_TIP_SCOPE);
    cleanup();
  });

  it("dismisses after a Vault-state change and removes its listener on cleanup", async () => {
    const { tips, items } = createTips();
    const target = new EventTarget();
    const fetchStatus = vi.fn()
      .mockResolvedValueOnce({ state: "locked" })
      .mockResolvedValueOnce({ state: "unlocked" });
    const cleanup = initializeStartupVaultTip({
      tips,
      t: (key) => key,
      fetchStatus,
      eventTarget: target,
    });
    await flush();
    expect(items).toHaveLength(1);

    target.dispatchEvent(new Event("norishell:vault-changed"));
    await flush();
    expect(items).toEqual([]);

    cleanup();
    target.dispatchEvent(new Event("norishell:vault-changed"));
    await flush();
    expect(fetchStatus).toHaveBeenCalledTimes(2);
  });

  it("does not reset the startup timer when later Vault events remain locked", async () => {
    const { tips, items } = createTips();
    const target = new EventTarget();
    const fetchStatus = vi.fn().mockResolvedValue({ state: "locked" });
    const cleanup = initializeStartupVaultTip({
      tips,
      t: (key) => key,
      fetchStatus,
      eventTarget: target,
    });
    await flush();
    target.dispatchEvent(new Event("norishell:vault-changed"));
    await flush();

    expect(items).toHaveLength(1);
    expect(tips.show).toHaveBeenCalledOnce();
    cleanup();
  });

  it("keeps the existing reminder after a cancelled secure action", async () => {
    const { tips, items } = createTips();
    const requestUnlock = vi.fn().mockResolvedValue(false);
    const cleanup = initializeStartupVaultTip({
      tips,
      t: (key) => key,
      fetchStatus: vi.fn().mockResolvedValue({ state: "locked" }),
      requestUnlock,
      eventTarget: new EventTarget(),
    });
    await flush();

    await items[0]?.action?.onClick();

    expect(requestUnlock).toHaveBeenCalledOnce();
    expect(items).toHaveLength(1);
    expect(tips.show).toHaveBeenCalledOnce();
    cleanup();
  });

  it("reports a non-secret failure when the explicit secure action rejects", async () => {
    const { tips, items } = createTips();
    const cleanup = initializeStartupVaultTip({
      tips,
      t: (key) => key,
      fetchStatus: vi.fn().mockResolvedValue({ state: "locked" }),
      requestUnlock: vi.fn().mockRejectedValue(new Error("fixture rejection")),
      eventTarget: new EventTarget(),
    });
    await flush();

    await items[0]?.action?.onClick();

    expect(items[0]).toEqual(expect.objectContaining({
      scope: "startup-vault-action",
      tone: "error",
      title: "tips.startupVault.unlockFailed",
    }));
    cleanup();
  });
});
