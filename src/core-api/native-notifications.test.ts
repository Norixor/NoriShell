import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { setNativeNotificationContext, testNativeNotification } from "./native-notifications";

describe("native notification IPC", () => {
  beforeEach(() => vi.clearAllMocks());

  it("sends locale without a renderer focus claim", async () => {
    await setNativeNotificationContext("en");
    expect(invoke).toHaveBeenCalledWith("native_notification_context_set", {
      request: { meta: { requestId: expect.any(String) }, locale: "en" },
    });
  });

  it("tests with locale only, with no completion event or terminal target", async () => {
    await testNativeNotification("zh-CN");
    expect(invoke).toHaveBeenCalledWith("native_notification_test", {
      request: { meta: { requestId: expect.any(String) }, locale: "zh-CN" },
    });
  });
});
