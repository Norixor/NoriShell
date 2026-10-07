import { DOMWrapper, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

import { handleMobileBack, resetMobileOverlaysForTests } from "../../mobile-overlays";
import NvxSelect from "./NvxSelect.vue";

const host = vi.hoisted(() => ({ mobile: false }));
vi.mock("../../platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../platform-host")>()),
  isMobileLayout: () => host.mobile,
}));

const options = [
  { value: "native", label: "Native" },
  { value: "unavailable", label: "Unavailable", disabled: true },
  { value: "remote", label: "Remote" },
];

afterEach(() => {
  document.body.innerHTML = "";
  host.mobile = false;
  resetMobileOverlaysForTests();
});

function mountSelect(props: Record<string, unknown> = {}) {
  const host = document.createElement("div");
  document.body.append(host);
  return mount(NvxSelect, {
    attachTo: host,
    props: {
      modelValue: "native",
      options,
      ...props,
    },
  });
}

function optionElements() {
  return [...document.body.querySelectorAll<HTMLElement>("[role='option']")].map(
    (element) => new DOMWrapper(element),
  );
}

describe("NvxSelect", () => {
  it("carries the secure theme boundary into its teleported listbox", async () => {
    const wrapper = mountSelect();
    wrapper.element.parentElement!.setAttribute("data-theme-protected", "");
    await wrapper.get("[role='combobox']").trigger("click");
    expect(document.body.querySelector("[role='listbox']")?.hasAttribute("data-theme-protected")).toBe(true);
    wrapper.unmount();
  });

  it("emits the selected controlled value from the custom listbox", async () => {
    const wrapper = mountSelect();
    await wrapper.get("[role='combobox']").trigger("click");

    const renderedOptions = optionElements();
    expect(renderedOptions).toHaveLength(3);
    expect(document.body.querySelector("select, option")).toBeNull();
    await renderedOptions[2]!.trigger("click");

    expect(wrapper.emitted("update:modelValue")).toEqual([["remote"]]);
    expect(wrapper.get("[role='combobox']").text()).toContain("Native");
  });

  it("navigates enabled options by keyboard and selects with Enter", async () => {
    const wrapper = mountSelect();
    const trigger = wrapper.get("[role='combobox']");

    await trigger.trigger("keydown", { key: "ArrowDown" });
    expect(trigger.attributes("aria-expanded")).toBe("true");
    await trigger.trigger("keydown", { key: "ArrowDown" });
    expect(trigger.attributes("aria-activedescendant")).toMatch(/option-2$/);
    await trigger.trigger("keydown", { key: "Home" });
    expect(trigger.attributes("aria-activedescendant")).toMatch(/option-0$/);
    await trigger.trigger("keydown", { key: "ArrowUp" });
    expect(trigger.attributes("aria-activedescendant")).toMatch(/option-2$/);
    await trigger.trigger("keydown", { key: "Home" });
    await trigger.trigger("keydown", { key: "End" });
    expect(trigger.attributes("aria-activedescendant")).toMatch(/option-2$/);
    await trigger.trigger("keydown", { key: "Enter" });

    expect(wrapper.emitted("update:modelValue")).toEqual([["remote"]]);
    expect(trigger.attributes("aria-expanded")).toBe("false");
  });

  it("closes on Escape and restores focus to the trigger", async () => {
    const wrapper = mountSelect();
    const trigger = wrapper.get<HTMLButtonElement>("[role='combobox']");
    await trigger.trigger("click");
    const outside = document.createElement("button");
    document.body.append(outside);
    outside.focus();
    expect(document.activeElement).toBe(outside);

    await trigger.trigger("keydown", { key: "Escape" });

    expect(trigger.attributes("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(trigger.element);
  });

  it("selects with Space and lets Tab move on after closing", async () => {
    const wrapper = mountSelect();
    const trigger = wrapper.get("[role='combobox']");

    await trigger.trigger("keydown", { key: "ArrowDown" });
    await trigger.trigger("keydown", { key: "ArrowDown" });
    await trigger.trigger("keydown", { key: " " });
    expect(wrapper.emitted("update:modelValue")).toEqual([["remote"]]);

    await trigger.trigger("keydown", { key: "Enter" });
    expect(trigger.attributes("aria-expanded")).toBe("true");
    await trigger.trigger("keydown", { key: "Tab" });
    expect(trigger.attributes("aria-expanded")).toBe("false");
    expect(wrapper.emitted("update:modelValue")).toEqual([["remote"]]);
  });

  it("does not open or emit when disabled", async () => {
    const wrapper = mountSelect({ disabled: true });
    const trigger = wrapper.get("[role='combobox']");

    expect(trigger.attributes("disabled")).toBeDefined();
    await trigger.trigger("click");
    await trigger.trigger("keydown", { key: "ArrowDown" });

    expect(trigger.attributes("aria-expanded")).toBe("false");
    expect(wrapper.emitted("update:modelValue")).toBeUndefined();
    expect(document.body.querySelector("[role='listbox']")).toBeNull();
  });

  it("closes when a pointer press starts outside the trigger and listbox", async () => {
    const wrapper = mountSelect();
    const trigger = wrapper.get("[role='combobox']");
    await trigger.trigger("click");
    expect(document.body.querySelector("[role='listbox']")).not.toBeNull();

    document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    await wrapper.vm.$nextTick();

    expect(trigger.attributes("aria-expanded")).toBe("false");
    expect(document.body.querySelector("[role='listbox']")).toBeNull();
  });

  it("renders a dialog select popup above the dialog layer", async () => {
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.append(dialog);
    const wrapper = mount(NvxSelect, {
      attachTo: dialog,
      props: { modelValue: "native", options },
    });

    await wrapper.get("[role='combobox']").trigger("click");

    expect(
      document.body.querySelector("[role='listbox']")?.classList.contains(
        "nvx-select__listbox--in-dialog",
      ),
    ).toBe(true);
  });

  it("keeps compact triggers from collapsing the popup options", async () => {
    const wrapper = mountSelect({ popupMinWidth: 160 });
    const trigger = wrapper.get<HTMLButtonElement>("[role='combobox']");
    trigger.element.getBoundingClientRect = () => ({
      x: 20,
      y: 20,
      top: 20,
      right: 52,
      bottom: 52,
      left: 20,
      width: 32,
      height: 32,
      toJSON: () => ({}),
    });

    await trigger.trigger("click");

    expect(document.body.querySelector<HTMLElement>("[role='listbox']")?.style.width).toBe("160px");
  });

  it("keeps the anchored listbox on desktop (no sheet, no search)", async () => {
    const wrapper = mountSelect();
    await wrapper.find(".nvx-select__trigger").trigger("click");
    expect(document.body.querySelector(".nvx-select__sheet")).toBeNull();
    expect(document.body.querySelector(".nvx-select__search")).toBeNull();
    expect(document.body.querySelector("[role='listbox']")?.getAttribute("style")).toContain("left");
  });

  it("opens a bottom sheet with a filter for long lists on the phone shell and closes on the back key", async () => {
    host.mobile = true;
    const many = Array.from({ length: 12 }, (_, index) => ({ value: `v${index}`, label: `Option ${index}` }));
    const wrapper = mountSelect({ modelValue: "v3", options: many, searchLabel: "Filter options" });
    await wrapper.find(".nvx-select__trigger").trigger("click");
    await wrapper.vm.$nextTick();
    const sheet = document.body.querySelector<HTMLElement>(".nvx-select__sheet")!;
    expect(sheet.hasAttribute("data-nvx-mobile-sheet")).toBe(true);
    expect(document.body.querySelector(".nvx-mobile-sheet-backdrop")).not.toBeNull();
    const search = sheet.querySelector<HTMLInputElement>(".nvx-select__search")!;
    expect(search.getAttribute("aria-label")).toBe("Filter options");
    expect(optionElements()).toHaveLength(12);
    search.value = "Option 1";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await wrapper.vm.$nextTick();
    expect(optionElements().map((option) => option.text())).toEqual(["Option 1", "Option 10", "Option 11"]);
    search.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    search.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await wrapper.vm.$nextTick();
    expect(wrapper.emitted("update:modelValue")?.at(-1)).toEqual(["v1"]);
    expect(document.body.querySelector(".nvx-mobile-sheet-backdrop")).toBeNull();

    await wrapper.find(".nvx-select__trigger").trigger("click");
    await wrapper.vm.$nextTick();
    expect(document.body.querySelector(".nvx-select__sheet")).not.toBeNull();
    expect(handleMobileBack()).toBe(true);
    await wrapper.vm.$nextTick();
    expect(document.body.querySelector(".nvx-select__sheet")).toBeNull();
    expect(document.body.querySelector(".nvx-mobile-sheet-backdrop")).toBeNull();
  });

  it("omits the filter for short lists on the phone shell", async () => {
    host.mobile = true;
    const wrapper = mountSelect();
    await wrapper.find(".nvx-select__trigger").trigger("click");
    await wrapper.vm.$nextTick();
    expect(document.body.querySelector(".nvx-select__sheet")).not.toBeNull();
    expect(document.body.querySelector(".nvx-select__search")).toBeNull();
    expect(optionElements()).toHaveLength(3);
  });
});
