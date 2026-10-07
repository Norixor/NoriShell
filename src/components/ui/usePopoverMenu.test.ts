import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h, ref, Teleport } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";

import { handleMobileBack, resetMobileOverlaysForTests } from "../../mobile-overlays";
import { usePopoverMenu } from "./usePopoverMenu";

const host = vi.hoisted(() => ({ mobile: false }));
vi.mock("../../platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../platform-host")>()),
  isMobileLayout: () => host.mobile,
}));

/** A minimal consumer: a trigger plus a Teleported menu panel, like the pane and SFTP menus. */
const Consumer = defineComponent({
  setup() {
    const menu = usePopoverMenu();
    return () => h("div", { ref: menu.rootRef }, [
      h("button", { ref: menu.triggerRef, type: "button", onClick: menu.toggleMenu }, "open"),
      h(Teleport, { to: "body" }, menu.open.value
        ? [h("div", { ref: menu.viewportPanelRef, role: "menu", style: menu.viewportPanelStyle.value, onKeydown: menu.handleMenuKeyDown }, [
          h("button", { type: "button", role: "menuitem" }, "first"),
          h("button", { type: "button", role: "menuitem" }, "second"),
        ])]
        : []),
    ]);
  },
});

/**
 * A consumer whose menu panel renders inline under an ancestor with `contain: layout paint`
 * (like the terminal view): on desktop the panel stays there; on the phone the sheet must
 * escape it, or the fixed sheet is trapped beneath the body-level scrim.
 */
const ContainedConsumer = defineComponent({
  setup() {
    const menu = usePopoverMenu();
    const clicked = ref("");
    return () => h("div", { class: "contained", style: "contain: layout paint; position: relative;" }, [
      h("div", { ref: menu.rootRef }, [
        h("button", { ref: menu.triggerRef, type: "button", onClick: menu.toggleMenu }, "open"),
        menu.open.value
          ? h("div", { role: "menu", onKeydown: menu.handleMenuKeyDown }, [
            h("button", { type: "button", role: "menuitem", onClick: () => { clicked.value = "first"; menu.closeMenu(true); } }, "first"),
            h("button", { type: "button", role: "menuitem" }, "second"),
          ])
          : null,
        h("span", { class: "sibling" }, "after"),
      ]),
      h("p", { class: "clicked" }, clicked.value),
    ]);
  },
});

afterEach(() => {
  document.body.innerHTML = "";
  host.mobile = false;
  resetMobileOverlaysForTests();
});

describe("usePopoverMenu", () => {
  it("keeps the anchored desktop popover without a scrim", async () => {
    const wrapper = mount(Consumer, { attachTo: document.body });
    await wrapper.find("button").trigger("click");
    await flushPromises();
    const panel = document.body.querySelector<HTMLElement>('[role="menu"]')!;
    expect(panel.hasAttribute("data-nvx-mobile-sheet")).toBe(false);
    expect(panel.style.top).not.toBe("");
    expect(document.body.querySelector(".nvx-mobile-sheet-backdrop")).toBeNull();
  });

  it("presents the menu as a bottom sheet with a scrim on the phone shell and closes it on the back key", async () => {
    host.mobile = true;
    const wrapper = mount(Consumer, { attachTo: document.body });
    await wrapper.find("button").trigger("click");
    await flushPromises();
    const panel = document.body.querySelector<HTMLElement>('[role="menu"]')!;
    expect(panel.hasAttribute("data-nvx-mobile-sheet")).toBe(true);
    expect(panel.style.visibility).toBe("");
    expect(document.body.querySelector(".nvx-mobile-sheet-backdrop")).not.toBeNull();
    expect(document.activeElement?.textContent).toBe("first");
    expect(handleMobileBack()).toBe(true);
    await flushPromises();
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(document.body.querySelector(".nvx-mobile-sheet-backdrop")).toBeNull();
    expect(document.activeElement?.textContent).toBe("open");
    expect(handleMobileBack()).toBe(false);
  });

  it("keeps a phone sheet open across the window resize caused by the soft keyboard closing, while the desktop popover still dismisses", async () => {
    host.mobile = true;
    const sheet = mount(Consumer, { attachTo: document.body });
    await sheet.find("button").trigger("click");
    await flushPromises();
    expect(document.body.querySelector("[role=menu]")).not.toBeNull();
    // Opening the sheet moves focus off the terminal; Android closes the keyboard and the window grows.
    window.dispatchEvent(new Event("resize"));
    await flushPromises();
    expect(document.body.querySelector("[role=menu]")).not.toBeNull();
    sheet.unmount();
    resetMobileOverlaysForTests();

    host.mobile = false;
    const popover = mount(Consumer, { attachTo: document.body });
    await popover.find("button").trigger("click");
    await flushPromises();
    expect(document.body.querySelector("[role=menu]")).not.toBeNull();
    window.dispatchEvent(new Event("resize"));
    await flushPromises();
    expect(document.body.querySelector("[role=menu]")).toBeNull();
    popover.unmount();
  });

  it("keeps an inline menu inside its contained ancestor on desktop", async () => {
    const wrapper = mount(ContainedConsumer, { attachTo: document.body });
    await wrapper.find("button").trigger("click");
    await flushPromises();
    const panel = document.body.querySelector<HTMLElement>('[role="menu"]')!;
    expect(panel.closest(".contained")).not.toBeNull();
    expect(panel.parentElement?.tagName).toBe("DIV");
    expect(document.body.querySelector(".nvx-mobile-sheet-backdrop")).toBeNull();
  });

  it("hosts an inline phone sheet under body so it escapes a contained ancestor and stays clickable", async () => {
    host.mobile = true;
    const wrapper = mount(ContainedConsumer, { attachTo: document.body });
    const container = wrapper.find(".contained").element;
    await wrapper.find("button").trigger("click");
    await flushPromises();
    const panel = document.body.querySelector<HTMLElement>('[role="menu"]')!;
    expect(panel.hasAttribute("data-nvx-mobile-sheet")).toBe(true);
    // Sheet and scrim are siblings under body: the same stacking context, the sheet above the scrim.
    expect(panel.parentElement).toBe(document.body);
    expect(container.contains(panel)).toBe(false);
    const backdrop = document.body.querySelector<HTMLElement>(".nvx-mobile-sheet-backdrop")!;
    expect(backdrop.parentElement).toBe(document.body);
    expect(document.activeElement?.textContent).toBe("first");
    // Pointer-down inside the hosted sheet is not "outside"; the item click runs and closes the menu.
    const first = panel.querySelector<HTMLButtonElement>('[role="menuitem"]')!;
    first.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    expect(document.body.querySelector('[role="menu"]')).not.toBeNull();
    first.click();
    await flushPromises();
    expect(wrapper.find(".clicked").text()).toBe("first");
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(document.body.querySelector(".nvx-mobile-sheet-backdrop")).toBeNull();
    // Nothing stays behind: no placeholder comment, the sibling after the menu is intact.
    expect(Array.from(container.querySelectorAll("*")).some((node) => node.getAttribute("role") === "menu")).toBe(false);
    expect(Array.from(container.querySelectorAll("div")).flatMap((node) => Array.from(node.childNodes)).some((node) => node.nodeType === Node.COMMENT_NODE && node.textContent === "nvx-mobile-sheet")).toBe(false);
    expect(wrapper.find(".sibling").text()).toBe("after");
    // Reopen and close through the back key: the same round trip, still consistent.
    await wrapper.find("button").trigger("click");
    await flushPromises();
    expect(document.body.querySelector('[role="menu"]')?.parentElement).toBe(document.body);
    expect(handleMobileBack()).toBe(true);
    await flushPromises();
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(wrapper.find(".sibling").text()).toBe("after");
    expect(document.activeElement?.textContent).toBe("open");
  });
});
