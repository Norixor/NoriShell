import { describe, expect, it, vi } from "vitest";

vi.mock("./views/OverviewView.vue", () => ({ default: {} }));
vi.mock("./views/SshTerminalView.vue", () => ({ default: {} }));

import { router } from "./router";

describe("application startup routing", () => {
  it("enters Terminal so the startup preference can choose welcome or history", async () => {
    await router.push("/overview");
    await router.push("/");

    expect(router.currentRoute.value.fullPath).toBe("/terminal");
  });

  it("recovers unknown startup routes through Terminal", async () => {
    await router.push("/missing-route");

    expect(router.currentRoute.value.fullPath).toBe("/terminal");
  });
});
