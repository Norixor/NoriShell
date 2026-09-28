import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import {
  retainWorkspaceTabViewSummaries,
  setPendingWorkspaceTabView,
  setWorkspaceTabViewSummary,
} from "../../workspace-tab-view-state";
import type { WorkspaceTabKind } from "../../workspace-tab-windows";
import NvxWorkspaceTabPlaceholder from "./NvxWorkspaceTabPlaceholder.vue";

function registerPending(id: string, kind: WorkspaceTabKind, route?: string) {
  setWorkspaceTabViewSummary({ id, viewLabel: `view-${id}`, kind, label: id, stateLabel: "", route });
  setPendingWorkspaceTabView(id);
}

describe("NvxWorkspaceTabPlaceholder", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    setPendingWorkspaceTabView(null);
    retainWorkspaceTabViewSummaries(new Set());
    vi.useRealTimers();
  });

  it("covers the shell only while a new Tab is rendering", async () => {
    const wrapper = mount(NvxWorkspaceTabPlaceholder, { global: { plugins: [i18n] } });
    expect(wrapper.find(".nvx-workspace-tab-placeholder").exists()).toBe(false);
    registerPending("tab-1", "terminal");
    await wrapper.vm.$nextTick();
    const skeleton = wrapper.get(".nvx-workspace-tab-placeholder__skeleton");
    expect(skeleton.attributes("aria-busy")).toBe("true");
    expect(skeleton.attributes("aria-hidden")).toBe("true");
    setPendingWorkspaceTabView(null);
    await wrapper.vm.$nextTick();
    expect(wrapper.find(".nvx-workspace-tab-placeholder").exists()).toBe(false);
    wrapper.unmount();
  });

  it.each<[WorkspaceTabKind, string | undefined, string]>([
    ["terminal", "/terminal", "terminal"],
    ["file", "/sftp", "file"],
    ["desktop", "/desktop", "desktop"],
    ["page", "/new", "new"],
    ["page", "/settings", "settings"],
    ["page", "/settings/identities", "settings"],
    ["page", "/overview", "grid"],
    ["page", "/hosts", "grid"],
    ["page", "/plugins", "grid"],
    ["page", "/plugin/demo/home", "page"],
  ])("picks the %s skeleton for route %s", async (kind, route, variant) => {
    const wrapper = mount(NvxWorkspaceTabPlaceholder, { global: { plugins: [i18n] } });
    registerPending("tab-variant", kind, route);
    await wrapper.vm.$nextTick();
    expect(wrapper.get(".nvx-workspace-tab-placeholder").attributes("data-variant")).toBe(variant);
    wrapper.unmount();
  });

  it("falls back to the generic page skeleton when the Tab summary is unknown", async () => {
    const wrapper = mount(NvxWorkspaceTabPlaceholder, { global: { plugins: [i18n] } });
    setPendingWorkspaceTabView("orphan");
    await wrapper.vm.$nextTick();
    expect(wrapper.get(".nvx-workspace-tab-placeholder").attributes("data-variant")).toBe("page");
    wrapper.unmount();
  });

  it("renders a terminal prompt with a cursor for terminal Tabs", async () => {
    const wrapper = mount(NvxWorkspaceTabPlaceholder, { global: { plugins: [i18n] } });
    registerPending("tab-term", "terminal");
    await wrapper.vm.$nextTick();
    expect(wrapper.find(".nvx-tab-skeleton__cursor").exists()).toBe(true);
    expect(wrapper.find(".nvx-tab-skeleton__terminal-toolbar").exists()).toBe(true);
    wrapper.unmount();
  });

  it("announces the opening caption only after a slow open", async () => {
    const wrapper = mount(NvxWorkspaceTabPlaceholder, { global: { plugins: [i18n] } });
    registerPending("tab-slow", "terminal");
    await wrapper.vm.$nextTick();
    const notice = wrapper.get('[role="status"]');
    expect(notice.attributes("aria-live")).toBe("polite");
    expect(notice.text()).toBe("");
    vi.advanceTimersByTime(999);
    await wrapper.vm.$nextTick();
    expect(notice.text()).toBe("");
    vi.advanceTimersByTime(1);
    await wrapper.vm.$nextTick();
    expect(notice.text()).toBe(i18n.global.t("workspaceTabs.opening"));
    expect(notice.classes()).toContain("nvx-workspace-tab-placeholder__notice--visible");
    wrapper.unmount();
  });

  it("resets the slow caption timer when a quick open finishes or another Tab starts", async () => {
    const wrapper = mount(NvxWorkspaceTabPlaceholder, { global: { plugins: [i18n] } });
    registerPending("tab-a", "terminal");
    await wrapper.vm.$nextTick();
    vi.advanceTimersByTime(700);
    setPendingWorkspaceTabView(null);
    await wrapper.vm.$nextTick();
    vi.advanceTimersByTime(1000);
    expect(wrapper.find('[role="status"]').exists()).toBe(false);

    registerPending("tab-b", "page", "/new");
    await wrapper.vm.$nextTick();
    vi.advanceTimersByTime(700);
    registerPending("tab-c", "page", "/hosts");
    await wrapper.vm.$nextTick();
    vi.advanceTimersByTime(600);
    await wrapper.vm.$nextTick();
    expect(wrapper.get('[role="status"]').text()).toBe("");
    vi.advanceTimersByTime(400);
    await wrapper.vm.$nextTick();
    expect(wrapper.get('[role="status"]').text()).toBe(i18n.global.t("workspaceTabs.opening"));
    wrapper.unmount();
  });

  it("clears the pending timer on unmount", async () => {
    const wrapper = mount(NvxWorkspaceTabPlaceholder, { global: { plugins: [i18n] } });
    registerPending("tab-unmount", "terminal");
    await wrapper.vm.$nextTick();
    wrapper.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
