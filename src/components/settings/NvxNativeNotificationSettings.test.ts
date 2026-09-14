import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { nativeNotificationsEn } from "../../locales/native-notifications";

const api = vi.hoisted(() => ({
  getNativeNotificationPermission: vi.fn(),
  requestNativeNotificationPermission: vi.fn(),
  testNativeNotification: vi.fn(),
}));
vi.mock("../../core-api/native-notifications", async (importOriginal) => ({
  ...await importOriginal<typeof import("../../core-api/native-notifications")>(),
  ...api,
}));

import NvxNativeNotificationSettings from "./NvxNativeNotificationSettings.vue";

function mountSettings() {
  return mount(NvxNativeNotificationSettings, {
    global: { plugins: [createI18n({ legacy: false, locale: "en", messages: { en: { nativeNotifications: nativeNotificationsEn } } })] },
  });
}

function button(wrapper: ReturnType<typeof mountSettings>, text: string) {
  const found = wrapper.findAll("button").find((item) => item.text().includes(text));
  if (!found) throw new Error(`Missing button: ${text}`);
  return found;
}

describe("native notification settings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.getNativeNotificationPermission.mockResolvedValue({ permission: "notDetermined", lastDelivery: "idle" });
    api.requestNativeNotificationPermission.mockResolvedValue({ permission: "granted", lastDelivery: "idle" });
    api.testNativeNotification.mockResolvedValue({ permission: "granted", lastDelivery: "accepted" });
  });

  it("only reads permission on mount and requires explicit permission and test clicks", async () => {
    const wrapper = mountSettings();
    await flushPromises();
    expect(api.requestNativeNotificationPermission).not.toHaveBeenCalled();
    expect(api.testNativeNotification).not.toHaveBeenCalled();
    expect(button(wrapper, "Send test").attributes("disabled")).toBeDefined();
    await button(wrapper, "Allow system").trigger("click");
    await flushPromises();
    expect(api.requestNativeNotificationPermission).toHaveBeenCalledOnce();
    expect(wrapper.text()).toContain("Notifications allowed by the system");
    await button(wrapper, "Send test").trigger("click");
    await flushPromises();
    expect(api.testNativeNotification).toHaveBeenCalledWith("en");
    expect(wrapper.text()).toContain("The system accepted the test notification. It will not navigate to a terminal.");
    wrapper.unmount();
  });

  it("shows denied and unavailable system facts without claiming permission was granted", async () => {
    api.getNativeNotificationPermission.mockResolvedValueOnce({ permission: "denied", lastDelivery: "permissionDenied" });
    const wrapper = mountSettings();
    await flushPromises();
    expect(wrapper.text()).toContain("Notifications disabled by the system");
    expect(wrapper.text()).toContain("system notification settings");
    expect(button(wrapper, "Send test").attributes("disabled")).toBeDefined();
    api.getNativeNotificationPermission.mockResolvedValueOnce({ permission: "unavailable", lastDelivery: "unavailable" });
    await button(wrapper, "Refresh permission").trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("System notifications are unavailable");
    expect(api.requestNativeNotificationPermission).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("reports uncertain test delivery and never silently retries", async () => {
    api.getNativeNotificationPermission.mockResolvedValue({ permission: "granted", lastDelivery: "idle" });
    api.testNativeNotification.mockRejectedValue({ code: "timeout", requestId: "id" });
    const wrapper = mountSettings();
    await flushPromises();
    await button(wrapper, "Send test").trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("Delivery is unconfirmed");
    expect(wrapper.text()).not.toContain("The system accepted the test notification.");
    expect(api.testNativeNotification).toHaveBeenCalledOnce();
    wrapper.unmount();
  });

  it("reports a paused test as suppressed instead of a stopped service or failed delivery", async () => {
    api.getNativeNotificationPermission.mockResolvedValue({ permission: "granted", lastDelivery: "idle" });
    api.testNativeNotification.mockResolvedValue({ permission: "granted", lastDelivery: "suppressed" });
    const wrapper = mountSettings();
    await flushPromises();
    await button(wrapper, "Send test").trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("Notifications are paused, so the test notification was not sent.");
    expect(wrapper.text()).not.toContain("The notification service has stopped.");
    expect(api.testNativeNotification).toHaveBeenCalledOnce();
    wrapper.unmount();
  });
});
