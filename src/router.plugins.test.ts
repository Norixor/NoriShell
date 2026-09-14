import { describe, expect, it } from "vitest";

import { router } from "./router";

describe("plugin route", () => {
  it("exposes Plugins as an unguarded first-level page", () => {
    const route = router.getRoutes().find((candidate) => candidate.path === "/plugins");
    expect(route).toBeDefined();
    expect(route?.beforeEnter).toBeUndefined();
    expect(router.getRoutes().some((candidate) => candidate.path === "/settings/plugins")).toBe(false);
  });
});
