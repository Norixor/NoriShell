import { flushPromises, mount } from "@vue/test-utils";
import { Ellipsis, Pencil, Trash2 } from "lucide-vue-next";
import { afterEach, describe, expect, it, vi } from "vitest";

import { resetMobileOverlaysForTests } from "../../mobile-overlays";
import NvxActionMenu from "./NvxActionMenu.vue";
import type { NvxActionMenuItem } from "./NvxActionMenu.vue";

const host = vi.hoisted(() => ({ mobile: false }));
vi.mock("../../platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../platform-host")>()),
  isMobileLayout: () => host.mobile,
}));

const items: NvxActionMenuItem[] = [
  { id: "edit", label: "Edit", icon: Pencil },
  { id: "archive", label: "Archive", disabled: true },
  { id: "delete", label: "Delete", icon: Trash2, danger: true },
];

function mountMenu(props: Partial<{ label: string; items: NvxActionMenuItem[]; disabled: boolean }> = {}) {
  const target = document.createElement("div");
  document.body.append(target);
  return mount(NvxActionMenu, {
    attachTo: target,
    props: { label: "Row actions", items, icon: Ellipsis, ...props },
  });
}

function menuItems() {
  return Array.from(document.body.querySelectorAll<HTMLButtonElement>('[role="menu"] [role="menuitem"]'));
}

afterEach(() => {
  document.body.innerHTML = "";
  host.mobile = false;
  resetMobileOverlaysForTests();
});

describe("NvxActionMenu", () => {
  it("renders a labelled trigger that announces a menu", () => {
    const wrapper = mountMenu();
    const trigger = wrapper.get("button");
    expect(trigger.attributes("aria-label")).toBe("Row actions");
    expect(trigger.attributes("aria-haspopup")).toBe("menu");
    expect(trigger.attributes("aria-expanded")).toBe("false");
    expect(document.querySelector('[role="menu"]')).toBeNull();
    wrapper.unmount();
  });

  it("renders the default overflow icon when no icon is passed", () => {
    const target = document.createElement("div");
    document.body.append(target);
    const wrapper = mount(NvxActionMenu, { attachTo: target, props: { label: "Row actions", items } });
    expect(wrapper.find("button svg").exists()).toBe(true);
    wrapper.unmount();
  });

  it("opens on click and lists every item as a menuitem", async () => {
    const wrapper = mountMenu();
    await wrapper.get("button").trigger("click");
    await flushPromises();
    expect(wrapper.get("button").attributes("aria-expanded")).toBe("true");
    const menu = document.querySelector('[role="menu"]');
    expect(menu?.getAttribute("aria-label")).toBe("Row actions");
    expect(menuItems().map((item) => item.textContent?.trim())).toEqual(["Edit", "Archive", "Delete"]);
    wrapper.unmount();
  });

  it("emits select with the item id and closes", async () => {
    const wrapper = mountMenu();
    await wrapper.get("button").trigger("click");
    await flushPromises();
    menuItems()[0]!.click();
    await flushPromises();
    expect(wrapper.emitted("select")).toEqual([["edit"]]);
    expect(document.querySelector('[role="menu"]')).toBeNull();
    wrapper.unmount();
  });

  it("does not select disabled items", async () => {
    const wrapper = mountMenu();
    await wrapper.get("button").trigger("click");
    await flushPromises();
    const archive = menuItems().find((item) => item.textContent?.trim() === "Archive")!;
    expect(archive.disabled).toBe(true);
    archive.click();
    await flushPromises();
    expect(wrapper.emitted("select")).toBeUndefined();
    wrapper.unmount();
  });

  it("marks danger items and separates them from the regular actions", async () => {
    const wrapper = mountMenu();
    await wrapper.get("button").trigger("click");
    await flushPromises();
    const [edit, , remove] = menuItems();
    expect(remove!.classList).toContain("nvx-action-menu__item--danger");
    expect(edit!.classList).not.toContain("nvx-action-menu__item--danger");
    expect(document.querySelectorAll('[role="menu"] [role="separator"]')).toHaveLength(1);
    wrapper.unmount();
  });

  it("keeps a disabled trigger closed", async () => {
    const wrapper = mountMenu({ disabled: true });
    await wrapper.get("button").trigger("click");
    await flushPromises();
    expect(document.querySelector('[role="menu"]')).toBeNull();
    wrapper.unmount();
  });

  it("presents the menu as a bottom sheet on the phone shell", async () => {
    host.mobile = true;
    const wrapper = mountMenu();
    await wrapper.get("button").trigger("click");
    await flushPromises();
    const menu = document.querySelector<HTMLElement>('[role="menu"]');
    expect(menu?.dataset.nvxMobileSheet).toBe("");
    menuItems()[2]!.click();
    await flushPromises();
    expect(wrapper.emitted("select")).toEqual([["delete"]]);
    expect(document.querySelector('[role="menu"]')).toBeNull();
    wrapper.unmount();
  });
});
