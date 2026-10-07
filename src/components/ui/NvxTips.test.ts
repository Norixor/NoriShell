import { mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import { pushMobileOverlay, resetMobileOverlaysForTests } from "../../mobile-overlays";
import { useTipsStore } from "../../stores/tips";
import NvxTips from "./NvxTips.vue";

const host = vi.hoisted(() => ({ mobile: false }));
vi.mock("../../platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../platform-host")>()),
  isMobileLayout: () => host.mobile,
}));

describe("NvxTips", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    host.mobile = false;
    resetMobileOverlaysForTests();
  });

  it("moves phone tips to the top edge while a sheet or dialog is open so they never cover its actions", async () => {
    i18n.global.locale.value = "en";
    host.mobile = true;
    const pinia = createPinia();
    const wrapper = mount(NvxTips, { attachTo: document.body, global: { plugins: [pinia, i18n] } });
    useTipsStore(pinia).show({ tone: "error", title: "Bad key", durationMs: 0 });
    await wrapper.vm.$nextTick();
    const region = document.body.querySelector<HTMLElement>(".nvx-tips--mobile")!;
    expect(region.classList.contains("nvx-tips--mobile-overlay")).toBe(false);
    const sheet = pushMobileOverlay(() => true);
    await wrapper.vm.$nextTick();
    expect(region.classList.contains("nvx-tips--mobile-overlay")).toBe(true);
    sheet.release();
    await wrapper.vm.$nextTick();
    expect(region.classList.contains("nvx-tips--mobile-overlay")).toBe(false);
    wrapper.unmount();
  });

  it("never applies the overlay placement on desktop", async () => {
    i18n.global.locale.value = "en";
    const wrapper = mount(NvxTips, { attachTo: document.body, global: { plugins: [createPinia(), i18n] } });
    const overlay = pushMobileOverlay(() => true);
    await wrapper.vm.$nextTick();
    expect(document.body.querySelector(".nvx-tips--mobile-overlay")).toBeNull();
    overlay.release();
    wrapper.unmount();
  });

  it("keeps the desktop region on desktop and swipes a tip away on the phone shell", async () => {
    i18n.global.locale.value = "en";
    const desktop = mount(NvxTips, { attachTo: document.body, global: { plugins: [createPinia(), i18n] } });
    expect(document.body.querySelector(".nvx-tips--mobile")).toBeNull();
    desktop.unmount();
    document.body.innerHTML = "";

    host.mobile = true;
    const pinia = createPinia();
    const wrapper = mount(NvxTips, { attachTo: document.body, global: { plugins: [pinia, i18n] } });
    const tips = useTipsStore(pinia);
    tips.show({ tone: "info", title: "Saved", durationMs: 0 });
    await wrapper.vm.$nextTick();
    expect(document.body.querySelector(".nvx-tips--mobile")).not.toBeNull();
    const item = document.body.querySelector<HTMLElement>(".nvx-tips__item")!;
    item.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 100, clientY: 10, button: 0 }));
    item.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, clientX: 130, clientY: 12 }));
    item.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, clientX: 200, clientY: 12 }));
    await wrapper.vm.$nextTick();
    expect(item.style.transform).toBe("translateX(100px)");
    item.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, clientX: 200, clientY: 12 }));
    await wrapper.vm.$nextTick();
    expect(tips.items).toHaveLength(0);
  });

  it("renders application feedback in the global top region and dismisses it", async () => {
    i18n.global.locale.value = "en";
    const pinia = createPinia();
    const wrapper = mount(NvxTips, {
      attachTo: document.body,
      global: { plugins: [pinia, i18n] },
    });
    const tips = useTipsStore(pinia);
    tips.show({ tone: "error", title: "Tunnel failed", durationMs: 0 });
    await wrapper.vm.$nextTick();

    const region = document.body.querySelector<HTMLElement>(".nvx-tips");
    const item = document.body.querySelector<HTMLElement>(".nvx-tips__item");
    expect(region?.getAttribute("aria-label")).toBe("Notifications");
    expect(region?.parentElement).toBe(document.body);
    expect(item?.getAttribute("role")).toBe("alert");
    expect(item?.textContent).toContain("Tunnel failed");

    document.body.querySelector<HTMLButtonElement>('.nvx-tips__dismiss')?.click();
    await wrapper.vm.$nextTick();
    expect(tips.items).toHaveLength(0);
    wrapper.unmount();
  });

  it("renders a shared action and prevents duplicate clicks while it runs", async () => {
    i18n.global.locale.value = "en";
    const pinia = createPinia();
    const wrapper = mount(NvxTips, {
      attachTo: document.body,
      global: { plugins: [pinia, i18n] },
    });
    let resolveAction: (() => void) | undefined;
    const action = vi.fn(() => new Promise<void>((resolve) => { resolveAction = resolve; }));
    const tips = useTipsStore(pinia);
    tips.show({
      tone: "warning",
      title: "Vault locked",
      durationMs: 0,
      action: { label: "Unlock now", onClick: action },
    });
    await wrapper.vm.$nextTick();

    const actionButton = document.body.querySelector<HTMLButtonElement>(".nvx-tips__action");
    expect(actionButton?.textContent).toContain("Unlock now");
    actionButton?.click();
    actionButton?.click();
    await wrapper.vm.$nextTick();
    expect(action).toHaveBeenCalledOnce();
    expect(actionButton?.disabled).toBe(true);

    resolveAction?.();
    await Promise.resolve();
    await wrapper.vm.$nextTick();
    expect(actionButton?.disabled).toBe(false);
    wrapper.unmount();
  });
});
