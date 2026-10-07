import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import { handleMobileBack, resetMobileOverlaysForTests } from "../../mobile-overlays";
import { useWorkspaceTabsStore, workspacePageTabForRoute, type TerminalHeaderController } from "../../stores/workspaceTabs";
import NvxMobileTabSwitcher from "./NvxMobileTabSwitcher.vue";

vi.mock("../../platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../platform-host")>()),
  isMobileLayout: () => true,
}));

async function mountSwitcher(path = "/terminal") {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/:pathMatch(.*)*", component: { template: "<div />" } }] });
  await router.push(path);
  // Teleport is stubbed so the sheet content renders under the wrapper; the overlay stack is unaffected.
  const wrapper = mount(NvxMobileTabSwitcher, {
    attachTo: document.body, props: { modelValue: true }, global: { plugins: [router, i18n], stubs: { Teleport: true } },
  });
  await flushPromises();
  return { wrapper, router };
}

describe("NvxMobileTabSwitcher", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    i18n.global.locale.value = "en";
    resetMobileOverlaysForTests();
  });
  afterEach(() => { document.body.innerHTML = ""; });

  it("lists New first, then every Tab with type, state, Pane count and a close action; close keeps the controller semantics", async () => {
    const store = useWorkspaceTabsStore();
    const controller: TerminalHeaderController = {
      activate: vi.fn(), close: vi.fn(() => false), closeMany: vi.fn(() => true), quickConnect: vi.fn(() => true),
      openTelnet: vi.fn(() => true), deactivate: vi.fn(), toggleQuickCommands: vi.fn(), createTab: vi.fn(),
      focusNativeSession: vi.fn(() => false), focusSshSession: vi.fn(() => false), focusLocalSession: vi.fn(() => false),
      focusTelnetSession: vi.fn(() => false),
    };
    store.registerTerminalController(controller);
    store.syncTerminalState({
      tabs: [
        { groupId: "t1", label: "web", stateLabel: "Connected · 2 Panes", stateTone: "success", terminalKind: "ssh", paneCount: 2 },
        { groupId: "t2", label: "zsh", stateLabel: "Running · 1 Pane", stateTone: "success", terminalKind: "local", paneCount: 1 },
      ],
      activeTabId: "t2", busy: false, activationBlocked: false, quickCommandsOpen: false,
    });
    store.createInitialFileTab("file:11111111-1111-4111-8111-111111111111", "local", null, "Local files");
    store.importPageTab(workspacePageTabForRoute("/new")!);
    const { wrapper, router } = await mountSwitcher("/terminal");
    const rows = wrapper.findAll(".nvx-mobile-switcher__row");
    expect(rows[0]!.text()).toBe("New tab");
    expect(rows.slice(1).map((row) => row.text().replace(/\s+/g, " "))).toEqual([
      "web Connected · 2 Panes", "zsh Running · 1 Pane", "Local files 1 Pane", "New Page Page",
    ]);
    expect(wrapper.findAll(".nvx-mobile-switcher__item--active")).toHaveLength(1);
    expect(wrapper.find(".nvx-mobile-switcher__item--active").text()).toContain("zsh");
    expect(wrapper.findAll(".nvx-mobile-switcher__dot--success")).toHaveLength(2);

    await wrapper.find('button[aria-label="Close web"]').trigger("click");
    await flushPromises();
    // The Terminal view decides (confirmation, running attachments); a declined close keeps the Tab.
    expect(controller.close).toHaveBeenCalledWith("t1");
    expect(wrapper.findAll(".nvx-mobile-switcher__item")).toHaveLength(4);

    await rows[1]!.trigger("click");
    await flushPromises();
    expect(controller.activate).toHaveBeenCalledWith("t1");
    expect(wrapper.emitted("update:modelValue")?.at(-1)).toEqual([false]);
    expect(router.currentRoute.value.path).toBe("/terminal");
  });

  it("is a bottom sheet the back key closes, and tells the user when no Tab is open", async () => {
    const { wrapper } = await mountSwitcher("/hosts");
    expect(wrapper.find(".nvx-dialog--sheet-tall").exists()).toBe(true);
    expect(wrapper.find(".nvx-mobile-switcher__empty").text()).toContain("No tabs are open");
    expect(handleMobileBack()).toBe(true);
    expect(wrapper.emitted("update:modelValue")?.at(-1)).toEqual([false]);
  });
});
