import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { setNativeNotificationContext, testNativeNotification } from "./native-notifications";

describe("native notification IPC", () => {
  beforeEach(() => vi.clearAllMocks());

  it("sends an exact scope and locale without a renderer focus claim", async () => {
    const scope = { kind: "local" as const, sessionId: "session", generation: "3", ptyId: "pty", paneId: "pane" };
    await setNativeNotificationContext("en", scope);
    expect(invoke).toHaveBeenCalledWith("native_notification_context_set", {
      request: { meta: { requestId: expect.any(String) }, locale: "en", visibleScope: scope },
    });
  });

  it("tests with locale only, with no completion event or terminal target", async () => {
    await testNativeNotification("zh-CN");
    expect(invoke).toHaveBeenCalledWith("native_notification_test", {
      request: { meta: { requestId: expect.any(String) }, locale: "zh-CN" },
    });
  });
});
