import { describe, expect, it } from "vitest";
import { decodeDesktopFrame, desktopButtons, desktopPointerButtons, desktopKey, reservedDesktopKey } from "./input";
describe("desktop wire boundary", () => {
  it("maps extended keys without passing packed E0 values", () => {
    expect(desktopKey({ code: "ArrowUp", key: "ArrowUp", location: 0 }, true)).toEqual({ kind: "key", scanCode: 0x148, keysym: 0xff52, down: true });
    expect(desktopKey({ code: "ControlRight", key: "Control", location: 2 }, false)).toEqual({ kind: "key", scanCode: 0x11d, keysym: 0xffe4, down: false });
  });
  it("translates browser right and middle buttons to protocol bit order", () => { expect(desktopButtons(2)).toBe(4); expect(desktopButtons(4)).toBe(2); expect(desktopButtons(7)).toBe(7); });
  it("preserves down and up transitions when native input omits the buttons mask", () => {
    expect(desktopPointerButtons({ type: "pointerdown", button: 0, buttons: 0 })).toBe(1);
    expect(desktopPointerButtons({ type: "pointerdown", button: 2, buttons: 0 })).toBe(4);
    expect(desktopPointerButtons({ type: "pointerup", button: 2, buttons: 2 })).toBe(0);
    expect(desktopPointerButtons({ type: "pointerup", button: 0, buttons: 3 })).toBe(4);
    expect(desktopPointerButtons({ type: "pointermove", button: -1, buttons: 5 })).toBe(3);
    expect(desktopPointerButtons({ type: "pointerdown", button: 4, buttons: 16 })).toBe(0);
  });
  it("reserves tab shortcuts on both platforms", () => { expect(reservedDesktopKey({ code: "Digit9", metaKey: true, ctrlKey: false })).toBe(true); expect(reservedDesktopKey({ code: "Digit1", metaKey: false, ctrlKey: true })).toBe(true); expect(reservedDesktopKey({ code: "KeyA", metaKey: false, ctrlKey: true })).toBe(false); });
  it("rejects stale, truncated, oversized or mismatched frames", () => {
    const frame = new ArrayBuffer(20), view = new DataView(frame); view.setBigUint64(0, 12n, true); view.setUint32(8, 1, true); view.setUint32(12, 1, true);
    expect(decodeDesktopFrame(frame, 12n)).toBeNull(); expect(decodeDesktopFrame(frame, 11n)?.rgba.length).toBe(4);
    expect(() => decodeDesktopFrame(frame.slice(0, 19), 0n)).toThrow("invalidFrame");
    view.setUint32(8, 100000, true); expect(() => decodeDesktopFrame(frame, 0n)).toThrow("invalidFrame");
    expect(() => decodeDesktopFrame(new ArrayBuffer(8), 0n)).toThrow("invalidFrame");
  });
});
