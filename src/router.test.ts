import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./views/OverviewView.vue", () => ({ default: {} }));
vi.mock("./views/SshTerminalView.vue", () => ({ default: {} }));

import { router } from "./router";
import { UI_PREFERENCES_KEY } from "./ui-preferences";

function startWith(stored: Record<string, unknown> | null) {
  localStorage.clear();
  if (stored) localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify(stored));
  // The UI store reads the stored preference when it is created.
  setActivePinia(createPinia());
}

describe("application startup routing", () => {
  beforeEach(() => startWith(null));

  it("opens Overview by default when the startup preference was never set", async () => {
    await router.push("/terminal");
    await router.push("/");

    expect(router.currentRoute.value.fullPath).toBe("/overview");
  });

  it("opens Overview when the startup preference selects it", async () => {
    startWith({ terminalStartupBehavior: "overview" });
    await router.push("/terminal");
    await router.push("/");

    expect(router.currentRoute.value.fullPath).toBe("/overview");
  });

  it.each(["welcome", "restoreHistory"])("keeps a stored %s choice on Terminal so it can show welcome or history", async (behavior) => {
    startWith({ terminalStartupBehavior: behavior });
    await router.push("/overview");
    await router.push("/");

    expect(router.currentRoute.value.fullPath).toBe("/terminal");
  });

  it("treats an unknown stored startup value as never set", async () => {
    startWith({ terminalStartupBehavior: "dashboard" });
    await router.push("/terminal");
    await router.push("/");

    expect(router.currentRoute.value.fullPath).toBe("/overview");
  });

  it("recovers unknown routes through Terminal regardless of the startup preference", async () => {
    await router.push("/missing-route");

    expect(router.currentRoute.value.fullPath).toBe("/terminal");
  });
});
