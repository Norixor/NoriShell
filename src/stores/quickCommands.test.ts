import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  MAX_QUICK_COMMAND_LENGTH,
  useQuickCommandsStore,
} from "./quickCommands";

describe("quick commands store", () => {
  beforeEach(() => {
    localStorage.clear();
    setActivePinia(createPinia());
    vi.restoreAllMocks();
  });

  it("persists ordinary command text and restores it in a fresh store", () => {
    const store = useQuickCommandsStore();

    expect(store.save({ label: "Disk", command: "df -h" })).toBe("saved");
    expect(store.commands).toHaveLength(1);

    setActivePinia(createPinia());
    const restored = useQuickCommandsStore();
    expect(restored.commands[0]).toMatchObject({ label: "Disk", command: "df -h" });
  });

  it("rejects high-risk secret patterns and overlong commands without truncation", () => {
    const store = useQuickCommandsStore();

    expect(store.save({
      label: "Secret",
      command: "TOKEN=ghp_123456789012345678901234",
    })).toBe("sensitive");
    expect(store.save({
      label: "Long",
      command: "x".repeat(MAX_QUICK_COMMAND_LENGTH + 1),
    })).toBe("too-long");
    expect(store.commands).toHaveLength(0);
  });

  it("keeps the previous state when local storage rejects a save or delete", () => {
    const store = useQuickCommandsStore();
    expect(store.save({ label: "Disk", command: "df -h" })).toBe("saved");
    const id = store.commands[0]?.id;
    expect(id).toBeTruthy();

    vi.spyOn(localStorage, "setItem").mockImplementation(() => {
      throw new DOMException("Quota exceeded", "QuotaExceededError");
    });

    expect(store.save({ label: "Memory", command: "free -m" })).toBe("storage-error");
    expect(store.commands.map((item) => item.label)).toEqual(["Disk"]);
    expect(store.remove(id!)).toBe("storage-error");
    expect(store.commands.map((item) => item.label)).toEqual(["Disk"]);
  });
});
