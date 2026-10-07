import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  handleMobileBack, installMobileBackHandler, mobileBackStackDepth, mobileOverlayDepth, mobileOverlayDepthRef,
  MOBILE_OVERLAY_Z_PROPERTY, mountMobileSheetBackdrop, pushMobileBackTarget, pushMobileOverlay, resetMobileOverlaysForTests,
} from "./mobile-overlays";

function zOf(element: HTMLElement) {
  return Number(element.style.getPropertyValue(MOBILE_OVERLAY_Z_PROPERTY));
}

describe("mobile overlay stack", () => {
  beforeEach(() => resetMobileOverlaysForTests());
  afterEach(() => {
    document.body.innerHTML = "";
    delete window.__norishellHandleBack;
  });

  it("returns false with nothing open and closes overlays topmost first", () => {
    expect(handleMobileBack()).toBe(false);
    const order: string[] = [];
    const sheet = pushMobileOverlay(() => { order.push("sheet"); sheet.release(); return true; });
    const dialog = pushMobileOverlay(() => { order.push("dialog"); dialog.release(); return true; });
    expect(mobileOverlayDepth()).toBe(2);
    expect(handleMobileBack()).toBe(true);
    expect(handleMobileBack()).toBe(true);
    expect(order).toEqual(["dialog", "sheet"]);
    expect(mobileOverlayDepth()).toBe(0);
    expect(handleMobileBack()).toBe(false);
  });

  it("lets a blocking overlay consume the key without closing, and tolerates double release", () => {
    const blocking = pushMobileOverlay(() => true);
    expect(handleMobileBack()).toBe(true);
    expect(mobileOverlayDepth()).toBe(1);
    blocking.release();
    blocking.release();
    expect(mobileOverlayDepth()).toBe(0);
  });

  it("installs and uninstalls the native back entry point", () => {
    const uninstall = installMobileBackHandler();
    expect(window.__norishellHandleBack).toBe(handleMobileBack);
    const closed = vi.fn(() => true);
    pushMobileOverlay(closed);
    expect(window.__norishellHandleBack!()).toBe(true);
    expect(closed).toHaveBeenCalledTimes(1);
    uninstall();
    expect(window.__norishellHandleBack).toBeUndefined();
  });

  it("stacks later overlays above earlier ones and re-levels after a release", async () => {
    const sheetScrim = mountMobileSheetBackdrop(() => undefined);
    const sheet = document.createElement("div");
    document.body.appendChild(sheet);
    const first = pushMobileOverlay(() => true, () => [sheetScrim.element, sheet]);
    expect(zOf(sheetScrim.element)).toBe(40);
    expect(zOf(sheet)).toBe(40);
    // A dialog opened from inside the sheet (e.g. a plugin host approval) lands one step above it.
    let backdrop: HTMLElement | null = null;
    const second = pushMobileOverlay(() => true, () => [backdrop]);
    backdrop = document.createElement("div");
    document.body.appendChild(backdrop);
    second.refresh();
    expect(zOf(backdrop)).toBe(42);
    const third = pushMobileOverlay(() => true);
    expect(mobileOverlayDepth()).toBe(3);
    // Releasing the middle entry moves nothing below it, and an entry without layers styles nothing.
    second.release();
    expect(backdrop.style.getPropertyValue(MOBILE_OVERLAY_Z_PROPERTY)).toBe("");
    expect(zOf(sheet)).toBe(40);
    first.release();
    expect(sheet.style.getPropertyValue(MOBILE_OVERLAY_Z_PROPERTY)).toBe("");
    third.release();
    sheetScrim();
    await Promise.resolve();
    expect(mobileOverlayDepth()).toBe(0);
  });

  it("styles elements that render after the push once the tick settles", async () => {
    let panel: HTMLElement | null = null;
    const handle = pushMobileOverlay(() => true, () => [panel]);
    panel = document.createElement("div");
    await Promise.resolve();
    expect(zOf(panel)).toBe(40);
    handle.release();
  });

  it("handles a navigation back target in stack order without counting it as an overlay", () => {
    const order: string[] = [];
    const detail = pushMobileBackTarget(() => { order.push("detail"); detail.release(); return true; });
    expect(mobileOverlayDepth()).toBe(0);
    expect(mobileOverlayDepthRef.value).toBe(0);
    expect(mobileBackStackDepth()).toBe(1);
    // A select opened inside the detail takes the stacking slot of the first overlay and closes first.
    let listbox: HTMLElement | null = null;
    const select = pushMobileOverlay(() => { order.push("select"); select.release(); return true; }, () => [listbox]);
    listbox = document.createElement("div");
    select.refresh();
    expect(zOf(listbox)).toBe(40);
    expect(mobileOverlayDepth()).toBe(1);
    expect(mobileOverlayDepthRef.value).toBe(1);
    expect(handleMobileBack()).toBe(true);
    expect(handleMobileBack()).toBe(true);
    expect(order).toEqual(["select", "detail"]);
    expect(mobileBackStackDepth()).toBe(0);
    expect(handleMobileBack()).toBe(false);
  });

  it("mounts a tap-to-dismiss scrim and removes it on release", () => {
    const dismiss = vi.fn();
    const remove = mountMobileSheetBackdrop(dismiss);
    const backdrop = document.body.querySelector<HTMLElement>(".nvx-mobile-sheet-backdrop");
    expect(backdrop).not.toBeNull();
    backdrop!.dispatchEvent(new Event("pointerdown", { bubbles: true, cancelable: true }));
    expect(dismiss).toHaveBeenCalledTimes(1);
    remove();
    expect(document.body.querySelector(".nvx-mobile-sheet-backdrop")).toBeNull();
  });
});
