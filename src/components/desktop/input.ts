import type { DesktopInputEvent } from "../../core-api/generated/core-api";
import { detectDesktopPlatform, type DesktopPlatform } from "../../platform";
// PC set-1 scancodes; E0 is represented by bit 8, never a packed 0xe0xx value.
const codes: Record<string, number> = { Numpad0: 82, Numpad1: 79, Numpad2: 80, Numpad3: 81, Numpad4: 75, Numpad5: 76, Numpad6: 77, Numpad7: 71, Numpad8: 72, Numpad9: 73, NumpadAdd: 78, NumpadSubtract: 74, NumpadMultiply: 55, NumpadDivide: 0x135, NumpadDecimal: 83, NumpadEnter: 0x11c, NumLock: 69, ScrollLock: 70, Escape: 1, Backspace: 14, Tab: 15, Enter: 28, ControlLeft: 29, ShiftLeft: 42, ShiftRight: 54, AltLeft: 56, Space: 57, CapsLock: 58, ControlRight: 0x11d, AltRight: 0x138, MetaLeft: 0x15b, MetaRight: 0x15c, ArrowUp: 0x148, ArrowLeft: 0x14b, ArrowRight: 0x14d, ArrowDown: 0x150, Home: 0x147, End: 0x14f, PageUp: 0x149, PageDown: 0x151, Insert: 0x152, Delete: 0x153, Minus: 12, Equal: 13, BracketLeft: 26, BracketRight: 27, Semicolon: 39, Quote: 40, Backquote: 41, Backslash: 43, Comma: 51, Period: 52, Slash: 53, IntlBackslash: 0x56, IntlRo: 0x73, IntlYen: 0x7d, ContextMenu: 0x15d, PrintScreen: 0x137 };
for (const [letters, start] of [["QWERTYUIOP", 16], ["ASDFGHJKL", 30], ["ZXCVBNM", 44]] as const) [...letters].forEach((letter, i) => { codes[`Key${letter}`] = start + i; });
[..."1234567890"].forEach((digit, i) => { codes[`Digit${digit}`] = i + 2; });
for (let i = 1; i <= 12; i++) codes[`F${i}`] = i <= 10 ? i + 58 : i + 76;
// F13–F23 continue at 0x64; F24 is 0x76 in set 1.
for (let i = 13; i <= 23; i++) codes[`F${i}`] = 0x64 + (i - 13);
codes.F24 = 0x76;
const keysyms: Record<string, number> = { NumLock: 0xff7f, ScrollLock: 0xff14, Backspace: 0xff08, Tab: 0xff09, Enter: 0xff0d, Escape: 0xff1b, Delete: 0xffff, Home: 0xff50, ArrowLeft: 0xff51, ArrowUp: 0xff52, ArrowRight: 0xff53, ArrowDown: 0xff54, PageUp: 0xff55, PageDown: 0xff56, End: 0xff57, Insert: 0xff63, Shift: 0xffe1, Control: 0xffe3, CapsLock: 0xffe5, Meta: 0xffeb, Alt: 0xffe9, ContextMenu: 0xff67, PrintScreen: 0xff61 };
/** Workspace Tab shortcuts stay in the app: ⌘+1…9 on macOS, Ctrl+1…9 elsewhere. */
export function reservedDesktopKey(event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey">, platform: DesktopPlatform = detectDesktopPlatform()) {
  return (platform === "macos" ? event.metaKey : event.ctrlKey) && /^Digit[1-9]$/.test(event.code);
}
export interface DesktopKeyOptions {
  /** macOS only: deliver the Command keys as the remote Control keys. */
  commandAsControl?: boolean;
}
export function desktopKey(event: Pick<KeyboardEvent, "code" | "key" | "location">, down: boolean, options: DesktopKeyOptions = {}): DesktopInputEvent | null {
  let code = event.code, key = event.key;
  if (options.commandAsControl && (code === "MetaLeft" || code === "MetaRight")) {
    code = code === "MetaLeft" ? "ControlLeft" : "ControlRight";
    if (key === "Meta") key = "Control";
  }
  const scanCode = codes[code];
  if (scanCode === undefined) return null;
  let keysym = keysyms[key] ?? (/^F\d+$/.test(key) ? 0xffbd + Number(key.slice(1)) : 0);
  if ([...key].length === 1) { const point = key.codePointAt(0)!; keysym = point <= 255 ? point : 0x1000000 + point; }
  if (event.location === 2 && ["Shift", "Control", "Meta", "Alt"].includes(key)) keysym += 1;
  return { kind: "key", scanCode, keysym, down };
}
export type DesktopSpecialKey = "ctrlAltDel" | "win" | "altTab" | "ctrlEsc" | "printScreen";
export const desktopSpecialKeys: readonly DesktopSpecialKey[] = ["ctrlAltDel", "win", "altTab", "ctrlEsc", "printScreen"];
const specialChords: Record<DesktopSpecialKey, readonly (readonly [code: string, key: string])[]> = {
  ctrlAltDel: [["ControlLeft", "Control"], ["AltLeft", "Alt"], ["Delete", "Delete"]],
  win: [["MetaLeft", "Meta"]],
  altTab: [["AltLeft", "Alt"], ["Tab", "Tab"]],
  ctrlEsc: [["ControlLeft", "Control"], ["Escape", "Escape"]],
  printScreen: [["PrintScreen", "PrintScreen"]],
};
/** Presses the chord keys in order and releases them in reverse; the caller sends the events sequentially. */
export function desktopSpecialKeySequence(id: DesktopSpecialKey): DesktopInputEvent[] {
  const keys = specialChords[id].map(([code, key]) => desktopKey({ code, key, location: 0 }, true)!);
  return [...keys, ...keys.map((event) => ({ ...event, down: false })).reverse()];
}
export function desktopButtons(buttons: number) { return (buttons & 1) | ((buttons & 4) >> 1) | ((buttons & 2) << 1); }
// Press and release events both provide the changed button; native assistive input may omit buttons.
// Move events retain the browser's complete state, while release clears only the changed button and preserves others.
export function desktopPointerButtons(event: Pick<PointerEvent, "type" | "buttons" | "button">) {
  const changed = event.button === 0 ? 1 : event.button === 1 ? 4 : event.button === 2 ? 2 : 0;
  const buttons = event.type === "pointerdown" ? event.buttons | changed
    : event.type === "pointerup" ? event.buttons & ~changed : event.buttons;
  return desktopButtons(buttons);
}
export const MAX_DESKTOP_FRAME_RECTS = 16;
export const MAX_DESKTOP_CURSOR_DIMENSION = 384;
export interface DesktopFrameRect { x: number; y: number; width: number; height: number; rgba: Uint8ClampedArray<ArrayBuffer> }
export interface DesktopFramePatch { base: bigint; width: number; height: number; rects: DesktopFrameRect[] }
export type DesktopCursorShape =
  | { kind: "default" }
  | { kind: "hidden" }
  | { kind: "bitmap"; width: number; height: number; hotspotX: number; hotspotY: number; rgba: Uint8ClampedArray<ArrayBuffer> };
export interface DecodedDesktopFrame {
  frameSequence: bigint;
  cursorSequence: bigint;
  /** Present only when the response carried a frame block; `null` when the block is stale for `after`. */
  frame: DesktopFramePatch | null;
  hasFrameBlock: boolean;
  cursor: DesktopCursorShape | null;
}
const HEADER = 24, FRAME_HEADER = 20, RECT = 16, CURSOR_HEADER = 20;
/**
 * Decodes the version 2 frame response. Every length is validated before any view is created, so a
 * malformed body throws `invalidFrame` instead of painting partial or misaligned pixels.
 */
export function decodeDesktopFrame(buffer: ArrayBuffer, after: bigint): DecodedDesktopFrame | null {
  if (!buffer.byteLength) return null;
  const invalid = () => new Error("invalidFrame");
  if (buffer.byteLength < HEADER) throw invalid();
  const view = new DataView(buffer);
  const version = view.getUint32(0, true), flags = view.getUint32(4, true);
  if (version !== 2 || (flags & ~0b11) !== 0) throw invalid();
  const frameSequence = view.getBigUint64(8, true), cursorSequence = view.getBigUint64(16, true);
  const hasFrameBlock = (flags & 1) !== 0, hasCursorBlock = (flags & 2) !== 0;
  let offset = HEADER;
  let payload = 0;
  let frameHeader: { base: bigint; width: number; height: number; rects: Omit<DesktopFrameRect, "rgba">[] } | null = null;
  if (hasFrameBlock) {
    if (buffer.byteLength < offset + FRAME_HEADER) throw invalid();
    const base = view.getBigUint64(offset, true);
    const width = view.getUint32(offset + 8, true), height = view.getUint32(offset + 12, true), rectCount = view.getUint32(offset + 16, true);
    offset += FRAME_HEADER;
    if (!width || !height || width > 8192 || height > 8192 || width * height > 16_777_216
      || rectCount < 1 || rectCount > MAX_DESKTOP_FRAME_RECTS || base >= frameSequence) throw invalid();
    if (buffer.byteLength < offset + rectCount * RECT) throw invalid();
    const rects: Omit<DesktopFrameRect, "rgba">[] = [];
    for (let index = 0; index < rectCount; index++) {
      const x = view.getUint32(offset, true), y = view.getUint32(offset + 4, true);
      const rectWidth = view.getUint32(offset + 8, true), rectHeight = view.getUint32(offset + 12, true);
      offset += RECT;
      if (!rectWidth || !rectHeight || x + rectWidth > width || y + rectHeight > height) throw invalid();
      payload += rectWidth * rectHeight * 4;
      rects.push({ x, y, width: rectWidth, height: rectHeight });
    }
    if (base === 0n && (rectCount !== 1 || rects[0]!.x !== 0 || rects[0]!.y !== 0 || rects[0]!.width !== width || rects[0]!.height !== height)) throw invalid();
    frameHeader = { base, width, height, rects };
  }
  let cursorHeader: { kind: number; width: number; height: number; hotspotX: number; hotspotY: number } | null = null;
  if (hasCursorBlock) {
    if (buffer.byteLength < offset + CURSOR_HEADER) throw invalid();
    const kind = view.getUint32(offset, true), width = view.getUint32(offset + 4, true), height = view.getUint32(offset + 8, true);
    const hotspotX = view.getUint32(offset + 12, true), hotspotY = view.getUint32(offset + 16, true);
    offset += CURSOR_HEADER;
    if (kind > 2) throw invalid();
    if (kind === 2) {
      if (!width || !height || width > MAX_DESKTOP_CURSOR_DIMENSION || height > MAX_DESKTOP_CURSOR_DIMENSION || hotspotX >= width || hotspotY >= height) throw invalid();
      payload += width * height * 4;
    } else if (width || height || hotspotX || hotspotY) throw invalid();
    cursorHeader = { kind, width, height, hotspotX, hotspotY };
  }
  if (buffer.byteLength !== offset + payload) throw invalid();
  let frame: DesktopFramePatch | null = null;
  if (frameHeader) {
    const rects: DesktopFrameRect[] = frameHeader.rects.map((rect) => {
      const length = rect.width * rect.height * 4;
      const rgba = new Uint8ClampedArray(buffer, offset, length);
      offset += length;
      return { ...rect, rgba };
    });
    // A frame block at or below the applied sequence carries nothing new; the cursor block may still apply.
    if (frameSequence > after) frame = { base: frameHeader.base, width: frameHeader.width, height: frameHeader.height, rects };
  }
  let cursor: DesktopCursorShape | null = null;
  if (cursorHeader) {
    if (cursorHeader.kind === 2) {
      cursor = { kind: "bitmap", width: cursorHeader.width, height: cursorHeader.height, hotspotX: cursorHeader.hotspotX, hotspotY: cursorHeader.hotspotY, rgba: new Uint8ClampedArray(buffer, offset, cursorHeader.width * cursorHeader.height * 4) };
    } else cursor = { kind: cursorHeader.kind === 1 ? "hidden" : "default" };
  }
  return { frameSequence, cursorSequence, frame, hasFrameBlock, cursor };
}
