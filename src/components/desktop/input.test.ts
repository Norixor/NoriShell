import { describe, expect, it } from "vitest";
import { decodeDesktopFrame, desktopButtons, desktopPointerButtons, desktopKey, desktopSpecialKeySequence, reservedDesktopKey } from "./input";

type Rect = readonly [x: number, y: number, width: number, height: number];
interface FrameSpec {
  frameSequence?: bigint;
  cursorSequence?: bigint;
  frame?: { base: bigint; width: number; height: number; rects?: Rect[] };
  cursor?: { kind: number; width?: number; height?: number; hotspotX?: number; hotspotY?: number };
  extraBytes?: number;
}
/** Builds a version 2 frame response; pixel payload is zero-filled. */
export function frameV2(spec: FrameSpec) {
  const rects = spec.frame ? spec.frame.rects ?? [[0, 0, spec.frame.width, spec.frame.height] as Rect] : [];
  const cursorPixels = spec.cursor?.kind === 2 ? (spec.cursor.width ?? 0) * (spec.cursor.height ?? 0) * 4 : 0;
  const payload = rects.reduce((sum, [, , w, h]) => sum + w * h * 4, 0) + cursorPixels;
  const length = 24 + (spec.frame ? 20 + rects.length * 16 : 0) + (spec.cursor ? 20 : 0) + payload + (spec.extraBytes ?? 0);
  const buffer = new ArrayBuffer(length), view = new DataView(buffer);
  view.setUint32(0, 2, true);
  view.setUint32(4, (spec.frame ? 1 : 0) | (spec.cursor ? 2 : 0), true);
  view.setBigUint64(8, spec.frameSequence ?? 0n, true);
  view.setBigUint64(16, spec.cursorSequence ?? 0n, true);
  let offset = 24;
  if (spec.frame) {
    view.setBigUint64(offset, spec.frame.base, true);
    view.setUint32(offset + 8, spec.frame.width, true); view.setUint32(offset + 12, spec.frame.height, true); view.setUint32(offset + 16, rects.length, true);
    offset += 20;
    for (const rect of rects) { rect.forEach((value, index) => view.setUint32(offset + index * 4, value, true)); offset += 16; }
  }
  if (spec.cursor) {
    [spec.cursor.kind, spec.cursor.width ?? 0, spec.cursor.height ?? 0, spec.cursor.hotspotX ?? 0, spec.cursor.hotspotY ?? 0]
      .forEach((value, index) => view.setUint32(offset + index * 4, value, true));
    offset += 20;
  }
  return buffer;
}

describe("desktop wire boundary", () => {
  it("maps extended keys without passing packed E0 values", () => {
    expect(desktopKey({ code: "ArrowUp", key: "ArrowUp", location: 0 }, true)).toEqual({ kind: "key", scanCode: 0x148, keysym: 0xff52, down: true });
    expect(desktopKey({ code: "ControlRight", key: "Control", location: 2 }, false)).toEqual({ kind: "key", scanCode: 0x11d, keysym: 0xffe4, down: false });
  });
  it("maps international, menu, print screen and F13–F24 keys", () => {
    expect(desktopKey({ code: "IntlBackslash", key: "§", location: 0 }, true)).toMatchObject({ scanCode: 0x56, keysym: 0xa7 });
    expect(desktopKey({ code: "IntlRo", key: "\\", location: 0 }, true)).toMatchObject({ scanCode: 0x73 });
    expect(desktopKey({ code: "IntlYen", key: "¥", location: 0 }, true)).toMatchObject({ scanCode: 0x7d });
    expect(desktopKey({ code: "ContextMenu", key: "ContextMenu", location: 0 }, true)).toMatchObject({ scanCode: 0x15d, keysym: 0xff67 });
    expect(desktopKey({ code: "PrintScreen", key: "PrintScreen", location: 0 }, true)).toMatchObject({ scanCode: 0x137, keysym: 0xff61 });
    expect(desktopKey({ code: "F13", key: "F13", location: 0 }, true)).toMatchObject({ scanCode: 0x64, keysym: 0xffca });
    expect(desktopKey({ code: "F23", key: "F23", location: 0 }, true)).toMatchObject({ scanCode: 0x6e, keysym: 0xffd4 });
    expect(desktopKey({ code: "F24", key: "F24", location: 0 }, true)).toMatchObject({ scanCode: 0x76, keysym: 0xffd5 });
  });
  it("sends Command as Control only when the device preference is enabled", () => {
    expect(desktopKey({ code: "MetaLeft", key: "Meta", location: 1 }, true)).toMatchObject({ scanCode: 0x15b, keysym: 0xffeb });
    expect(desktopKey({ code: "MetaLeft", key: "Meta", location: 1 }, true, { commandAsControl: true })).toEqual({ kind: "key", scanCode: 29, keysym: 0xffe3, down: true });
    expect(desktopKey({ code: "MetaRight", key: "Meta", location: 2 }, false, { commandAsControl: true })).toEqual({ kind: "key", scanCode: 0x11d, keysym: 0xffe4, down: false });
    expect(desktopKey({ code: "KeyC", key: "c", location: 0 }, true, { commandAsControl: true })).toMatchObject({ scanCode: 46, keysym: 0x63 });
  });
  it("builds special key chords that press in order and release in reverse", () => {
    const sequence = desktopSpecialKeySequence("ctrlAltDel");
    expect(sequence.map((event) => event.kind === "key" ? [event.scanCode, event.down] : null)).toEqual([[29, true], [56, true], [0x153, true], [0x153, false], [56, false], [29, false]]);
    expect(desktopSpecialKeySequence("win")).toEqual([{ kind: "key", scanCode: 0x15b, keysym: 0xffeb, down: true }, { kind: "key", scanCode: 0x15b, keysym: 0xffeb, down: false }]);
    expect(desktopSpecialKeySequence("printScreen")[0]).toMatchObject({ scanCode: 0x137, keysym: 0xff61 });
    expect(desktopSpecialKeySequence("altTab")).toHaveLength(4);
    expect(desktopSpecialKeySequence("ctrlEsc")[1]).toMatchObject({ scanCode: 1, keysym: 0xff1b });
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
  it("reserves only the platform's own workspace tab shortcut", () => {
    expect(reservedDesktopKey({ code: "Digit9", metaKey: true, ctrlKey: false }, "macos")).toBe(true);
    expect(reservedDesktopKey({ code: "Digit1", metaKey: false, ctrlKey: true }, "macos")).toBe(false);
    expect(reservedDesktopKey({ code: "Digit1", metaKey: false, ctrlKey: true }, "windows")).toBe(true);
    expect(reservedDesktopKey({ code: "Digit1", metaKey: true, ctrlKey: false }, "windows")).toBe(false);
    expect(reservedDesktopKey({ code: "Digit5", metaKey: false, ctrlKey: true }, "other")).toBe(true);
    expect(reservedDesktopKey({ code: "KeyA", metaKey: false, ctrlKey: true }, "windows")).toBe(false);
  });
  it("decodes a full frame, a multi-rect patch and cursor blocks", () => {
    const full = decodeDesktopFrame(frameV2({ frameSequence: 12n, cursorSequence: 3n, frame: { base: 0n, width: 2, height: 2 } }), 11n);
    expect(full).toMatchObject({ frameSequence: 12n, cursorSequence: 3n, cursor: null, hasFrameBlock: true });
    expect(full?.frame).toMatchObject({ base: 0n, width: 2, height: 2 });
    expect(full?.frame?.rects.map((rect) => [rect.x, rect.y, rect.width, rect.height, rect.rgba.length])).toEqual([[0, 0, 2, 2, 16]]);
    const patch = decodeDesktopFrame(frameV2({ frameSequence: 13n, frame: { base: 12n, width: 4, height: 4, rects: [[0, 0, 1, 1], [3, 3, 1, 1]] }, cursor: { kind: 2, width: 2, height: 1, hotspotX: 1, hotspotY: 0 }, cursorSequence: 4n }), 12n);
    expect(patch?.frame?.rects).toHaveLength(2);
    expect(patch?.frame?.rects[1]).toMatchObject({ x: 3, y: 3, width: 1, height: 1 });
    expect(patch?.frame?.rects[1]?.rgba.byteOffset).toBe(24 + 20 + 32 + 20 + 4);
    expect(patch?.cursor).toMatchObject({ kind: "bitmap", width: 2, height: 1, hotspotX: 1, hotspotY: 0 });
    expect((patch?.cursor as { rgba: Uint8ClampedArray }).rgba.length).toBe(8);
    expect(decodeDesktopFrame(frameV2({ cursorSequence: 5n, cursor: { kind: 1 } }), 0n)).toMatchObject({ frame: null, hasFrameBlock: false, cursor: { kind: "hidden" } });
    expect(decodeDesktopFrame(frameV2({ cursorSequence: 6n, cursor: { kind: 0 } }), 0n)?.cursor).toEqual({ kind: "default" });
    expect(decodeDesktopFrame(new ArrayBuffer(0), 0n)).toBeNull();
  });
  it("ignores a stale frame block but still applies its cursor block", () => {
    const decoded = decodeDesktopFrame(frameV2({ frameSequence: 12n, cursorSequence: 2n, frame: { base: 0n, width: 1, height: 1 }, cursor: { kind: 1 } }), 12n);
    expect(decoded).toMatchObject({ frame: null, hasFrameBlock: true, cursor: { kind: "hidden" } });
  });
  it("rejects wrong versions, unknown flags, truncated, oversized and mismatched frames", () => {
    const bad = (spec: FrameSpec, mutate?: (view: DataView) => void) => {
      const buffer = frameV2(spec); mutate?.(new DataView(buffer));
      expect(() => decodeDesktopFrame(buffer, 0n)).toThrow("invalidFrame");
    };
    bad({ frameSequence: 1n, frame: { base: 0n, width: 1, height: 1 } }, (view) => view.setUint32(0, 1, true));
    bad({ frameSequence: 1n, frame: { base: 0n, width: 1, height: 1 } }, (view) => view.setUint32(4, 5, true));
    expect(() => decodeDesktopFrame(frameV2({ frameSequence: 1n, frame: { base: 0n, width: 1, height: 1 } }).slice(0, 47), 0n)).toThrow("invalidFrame");
    bad({ frameSequence: 1n, frame: { base: 0n, width: 1, height: 1 }, extraBytes: 1 });
    bad({ frameSequence: 1n, frame: { base: 0n, width: 100000, height: 1 } });
    bad({ frameSequence: 1n, frame: { base: 0n, width: 1, height: 1 } }, (view) => view.setUint32(24 + 16, 0, true));
    bad({ frameSequence: 5n, frame: { base: 5n, width: 2, height: 2, rects: [[0, 0, 1, 1]] } });
    bad({ frameSequence: 5n, frame: { base: 4n, width: 2, height: 2, rects: [[2, 0, 1, 1]] } });
    bad({ frameSequence: 5n, frame: { base: 4n, width: 2, height: 2, rects: [[0, 0, 0, 1]] } });
    bad({ frameSequence: 5n, frame: { base: 0n, width: 2, height: 2, rects: [[0, 0, 1, 1]] } });
    bad({ frameSequence: 5n, frame: { base: 0n, width: 2, height: 2, rects: Array.from({ length: 17 }, () => [0, 0, 1, 1] as Rect) } });
    bad({ cursorSequence: 1n, cursor: { kind: 3 } });
    bad({ cursorSequence: 1n, cursor: { kind: 0, width: 1 } });
    bad({ cursorSequence: 1n, cursor: { kind: 2, width: 385, height: 1 } });
    bad({ cursorSequence: 1n, cursor: { kind: 2, width: 2, height: 2, hotspotX: 2 } });
    expect(() => decodeDesktopFrame(new ArrayBuffer(8), 0n)).toThrow("invalidFrame");
  });
});
