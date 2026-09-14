import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h, nextTick, ref } from "vue";
import { afterEach, describe, expect, it } from "vitest";

import NvxDialog from "./NvxDialog.vue";

afterEach(() => {
  document.body.innerHTML = "";
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
});
