import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h, nextTick, ref } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";

import { handleMobileBack, mobileOverlayDepth, resetMobileOverlaysForTests } from "../../mobile-overlays";
import NvxDialog from "./NvxDialog.vue";

const host = vi.hoisted(() => ({ mobile: false }));
vi.mock("../../platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../platform-host")>()),
  isMobileLayout: () => host.mobile,
}));

afterEach(() => {
  document.body.innerHTML = "";
  host.mobile = false;
  resetMobileOverlaysForTests();
});

describe("NvxDialog", () => {
  it("separates theme protection from ordinary plugin DOM protection", async () => {
    const wrapper = mount(NvxDialog, { attachTo: document.body, props: {
      modelValue: true, title: "Sensitive decision", closeLabel: "Close", pluginProtected: true,
    } });
    await flushPromises();
    const backdrop = document.body.querySelector(".nvx-dialog__backdrop")!;
    expect(backdrop.hasAttribute("data-plugin-protected")).toBe(true);
    expect(backdrop.hasAttribute("data-theme-protected")).toBe(true);
    await wrapper.setProps({ themeProtected: false });
    expect(backdrop.hasAttribute("data-plugin-protected")).toBe(true);
    expect(backdrop.hasAttribute("data-theme-protected")).toBe(false);
    wrapper.unmount();
  });

  it("applies the requested dialog width", async () => {
    mount(NvxDialog, {
      attachTo: document.body,
      props: {
        modelValue: true,
        title: "Wide settings",
        closeLabel: "Close dialog",
        size: "lg",
      },
    });
    await flushPromises();

    expect(document.body.querySelector('[role="dialog"]')?.classList.contains("nvx-dialog--lg"))
      .toBe(true);
  });

  it("places split navigation beside the content and focuses its requested field", async () => {
    mount(NvxDialog, {
      attachTo: document.body,
      props: { modelValue: true, title: "History", closeLabel: "Close", layout: "split" },
      slots: {
        sidebar: () => h("button", "Current host"),
        default: () => h("input", { "data-nvx-dialog-initial-focus": "", "aria-label": "Search history" }),
        actions: () => h("button", "Done"),
      },
    });
    await flushPromises();

    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    expect(dialog?.classList.contains("nvx-dialog--split")).toBe(true);
    expect(dialog?.classList.contains("nvx-dialog--md")).toBe(false);
    expect(dialog?.querySelector(".nvx-dialog__sidebar")?.textContent).toContain("Current host");
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Search history");
  });

  it("announces itself, focuses the requested action, and traps tab focus", async () => {
    mount(NvxDialog, {
      attachTo: document.body,
      props: {
        modelValue: true,
        title: "Terminal decision",
        description: "Choose what happens next.",
        closeLabel: "Close dialog",
      },
      slots: {
        actions: () => [
          h("button", { "data-nvx-dialog-initial-focus": "" }, "Keep"),
          h("button", "Terminate"),
        ],
      },
    });
    await flushPromises();

    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    const buttons = Array.from(dialog!.querySelectorAll<HTMLButtonElement>("button"));
    expect(dialog?.getAttribute("aria-modal")).toBe("true");
    expect(dialog?.getAttribute("aria-labelledby")).toBeTruthy();
    expect(dialog?.getAttribute("aria-describedby")).toBeTruthy();
    expect(document.activeElement?.textContent).toBe("Keep");

    buttons.at(-1)!.focus();
    dialog!.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    await nextTick();
    expect(document.activeElement).toBe(buttons[0]);
    buttons[0]!.focus();
    dialog!.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true }),
    );
    await nextTick();
    expect(document.activeElement).toBe(buttons.at(-1));
  });

  it("uses the configured Escape policy and restores the invoking focus", async () => {
    const trigger = document.createElement("button");
    trigger.textContent = "Open";
    document.body.append(trigger);
    trigger.focus();

    const host = defineComponent({
      setup() {
        const open = ref(true);
        return () => h(NvxDialog, {
          modelValue: open.value,
          "onUpdate:modelValue": (value: boolean) => {
            open.value = value;
          },
          title: "Decision",
          closeLabel: "Close dialog",
        }, { actions: () => h("button", "Continue") });
      },
    });
    mount(host, { attachTo: document.body });
    await flushPromises();
    document.body.querySelector<HTMLElement>('[role="dialog"]')!.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    await flushPromises();

    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("keeps a non-dismissible decision open when Escape or the backdrop is used", async () => {
    const wrapper = mount(NvxDialog, {
      attachTo: document.body,
      props: {
        modelValue: true,
        title: "Required decision",
        closeLabel: "Close dialog",
        dismissible: false,
      },
      slots: { actions: () => h("button", "Decide") },
    });
    await flushPromises();

    document.body.querySelector<HTMLElement>('[role="dialog"]')!.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    document.body.querySelector<HTMLElement>(".nvx-dialog__backdrop")!.dispatchEvent(
      new MouseEvent("mousedown", { bubbles: true }),
    );

    expect(wrapper.emitted("close")).toBeUndefined();
    expect(document.body.querySelector('[role="dialog"]')).not.toBeNull();
  });

  it("keeps the desktop presentation (no sheet, no handle) while still answering the back key", async () => {
    const wrapper = mount(NvxDialog, { attachTo: document.body, props: { modelValue: true, title: "Desktop", closeLabel: "Close" } });
    await flushPromises();
    expect(document.body.querySelector(".nvx-dialog__backdrop--sheet")).toBeNull();
    expect(document.body.querySelector(".nvx-dialog--sheet")).toBeNull();
    expect(document.body.querySelector(".nvx-dialog__handle")).toBeNull();
    expect(mobileOverlayDepth()).toBe(1);
    expect(handleMobileBack()).toBe(true);
    expect(wrapper.emitted("update:modelValue")?.at(-1)).toEqual([false]);
    wrapper.unmount();
    expect(mobileOverlayDepth()).toBe(0);
  });

  it("presents a bottom sheet on the phone shell, closes on the back key and on a downward swipe of the handle", async () => {
    host.mobile = true;
    const wrapper = mount(NvxDialog, { attachTo: document.body, props: { modelValue: true, title: "Sheet", closeLabel: "Close", size: "xl" } });
    await flushPromises();
    expect(document.body.querySelector(".nvx-dialog__backdrop--sheet")).not.toBeNull();
    const panel = document.body.querySelector<HTMLElement>(".nvx-dialog--sheet")!;
    expect(panel.classList.contains("nvx-dialog--sheet-full")).toBe(true);
    const handle = document.body.querySelector<HTMLElement>(".nvx-dialog__handle")!;
    handle.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientY: 100, button: 0 }));
    handle.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, clientY: 160 }));
    await nextTick();
    expect(panel.style.transform).toBe("translateY(60px)");
    // Synthetic events share one timestamp, so only the distance rule can apply here: 20px is a tap-sized drift.
    handle.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, clientY: 120 }));
    await nextTick();
    expect(panel.style.transform).toBe("");
    expect(wrapper.emitted("update:modelValue")).toBeUndefined();
    handle.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientY: 100, button: 0 }));
    handle.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, clientY: 260 }));
    expect(wrapper.emitted("update:modelValue")?.at(-1)).toEqual([false]);
    await wrapper.setProps({ modelValue: true });
    await flushPromises();
    expect(handleMobileBack()).toBe(true);
    expect(wrapper.emitted("update:modelValue")).toHaveLength(2);
  });

  it("never autofocuses a text field inside a phone sheet (the soft keyboard would cover it)", async () => {
    host.mobile = true;
    const wrapper = mount(NvxDialog, {
      attachTo: document.body,
      props: { modelValue: true, title: "Search", closeLabel: "Close" },
      slots: { default: '<input data-nvx-dialog-initial-focus aria-label="Query"><button type="button">Go</button>' },
    });
    await flushPromises();
    const panel = document.body.querySelector<HTMLElement>(".nvx-dialog--sheet")!;
    expect(document.activeElement).toBe(panel);
    expect(document.activeElement?.tagName).not.toBe("INPUT");
    wrapper.unmount();
    // Desktop keeps the requested field focus.
    host.mobile = false;
    const desktop = mount(NvxDialog, {
      attachTo: document.body,
      props: { modelValue: true, title: "Search", closeLabel: "Close" },
      slots: { default: '<input data-nvx-dialog-initial-focus aria-label="Query">' },
    });
    await flushPromises();
    expect(document.activeElement?.tagName).toBe("INPUT");
    desktop.unmount();
  });

  it("lets a blocking sheet consume the back key without closing", async () => {
    host.mobile = true;
    const wrapper = mount(NvxDialog, { attachTo: document.body, props: { modelValue: true, title: "Busy", closeLabel: "Close", dismissible: false } });
    await flushPromises();
    expect(document.body.querySelector(".nvx-dialog__handle--static")).not.toBeNull();
    expect(handleMobileBack()).toBe(true);
    expect(wrapper.emitted("update:modelValue")).toBeUndefined();
    expect(document.body.querySelector('[role="dialog"]')).not.toBeNull();
  });
});
