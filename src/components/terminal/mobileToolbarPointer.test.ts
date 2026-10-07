import { afterEach, describe, expect, it, vi } from "vitest";

import { keepTerminalFocusOnPointerDown } from "./mobileToolbarPointer";

const host = vi.hoisted(() => ({ mobile: false }));
vi.mock("../../platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../platform-host")>()),
  isMobileLayout: () => host.mobile,
}));

function pointerDownOn(element: Element) {
  const event = new Event("pointerdown", { bubbles: true, cancelable: true });
  element.dispatchEvent(event);
  return event;
}

describe("keepTerminalFocusOnPointerDown", () => {
  afterEach(() => {
    host.mobile = false;
    document.body.innerHTML = "";
  });

  it("cancels toolbar button pointer-downs on the phone so the terminal keeps focus, but never for text fields", () => {
    host.mobile = true;
    document.body.innerHTML = '<header><button type="button">History</button><input aria-label="Search"></header>';
    const header = document.querySelector("header")!;
    header.addEventListener("pointerdown", keepTerminalFocusOnPointerDown);
    expect(pointerDownOn(header.querySelector("button")!).defaultPrevented).toBe(true);
    expect(pointerDownOn(header.querySelector("input")!).defaultPrevented).toBe(false);
  });

  it("leaves desktop pointer behavior untouched", () => {
    document.body.innerHTML = "<header><button type=\"button\">History</button></header>";
    const header = document.querySelector("header")!;
    header.addEventListener("pointerdown", keepTerminalFocusOnPointerDown);
    expect(pointerDownOn(header.querySelector("button")!).defaultPrevented).toBe(false);
  });
});
