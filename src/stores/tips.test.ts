import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useTipsStore } from "./tips";

describe("useTipsStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("replaces feedback from the same scope and keeps at most three tips", () => {
    const tips = useTipsStore();
    tips.show({ scope: "tunnels", title: "Saved" });
    tips.show({ scope: "tunnels", tone: "success", title: "Started" });
    tips.show({ scope: "copy", title: "Copied" });
    tips.show({ scope: "overview", tone: "error", title: "Retry failed" });
    tips.show({ scope: "settings", title: "Updated" });

    expect(tips.items).toHaveLength(3);
    expect(tips.items.map((item) => item.title)).toEqual(["Updated", "Retry failed", "Copied"]);
    expect(tips.items.some((item) => item.title === "Saved")).toBe(false);
  });

  it("auto dismisses by tone and supports persistent tips", () => {
    const tips = useTipsStore();
    tips.show({ scope: "success", tone: "success", title: "Saved" });
    tips.show({ scope: "persistent", tone: "warning", title: "Review", durationMs: 0 });

    vi.advanceTimersByTime(4_000);

    expect(tips.items.map((item) => item.title)).toEqual(["Review"]);
  });
});
