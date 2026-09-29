import { afterEach, describe, expect, it, vi } from "vitest";

import { installLinuxWindowShape } from "./linux-window-shape";

function stubUserAgent(value: string) {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(value);
}

describe("installLinuxWindowShape", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    delete document.documentElement.dataset.nvxShape;
  });

  it("marks Linux windows and removes the mark on dispose", () => {
    stubUserAgent("Mozilla/5.0 (X11; Linux aarch64)");
    const dispose = installLinuxWindowShape("window");
    expect(document.documentElement.dataset.nvxShape).toBe("window");
    dispose();
    expect(document.documentElement.dataset.nvxShape).toBeUndefined();
  });

  it("leaves macOS and Windows untouched", () => {
    for (const agent of ["Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)"]) {
      stubUserAgent(agent);
      installLinuxWindowShape("window");
      expect(document.documentElement.dataset.nvxShape).toBeUndefined();
      vi.restoreAllMocks();
    }
  });
});
