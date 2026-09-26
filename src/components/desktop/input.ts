import type { DesktopInputEvent } from "../../core-api/generated/core-api";
// PC set-1 scancodes; E0 is represented by bit 8, never a packed 0xe0xx value.
const codes: Record<string, number> = { Numpad0: 82, Numpad1: 79, Numpad2: 80, Numpad3: 81, Numpad4: 75, Numpad5: 76, Numpad6: 77, Numpad7: 71, Numpad8: 72, Numpad9: 73, NumpadAdd: 78, NumpadSubtract: 74, NumpadMultiply: 55, NumpadDivide: 0x135, NumpadDecimal: 83, NumpadEnter: 0x11c, NumLock: 69, ScrollLock: 70, Escape: 1, Backspace: 14, Tab: 15, Enter: 28, ControlLeft: 29, ShiftLeft: 42, ShiftRight: 54, AltLeft: 56, Space: 57, CapsLock: 58, ControlRight: 0x11d, AltRight: 0x138, MetaLeft: 0x15b, MetaRight: 0x15c, ArrowUp: 0x148, ArrowLeft: 0x14b, ArrowRight: 0x14d, ArrowDown: 0x150, Home: 0x147, End: 0x14f, PageUp: 0x149, PageDown: 0x151, Insert: 0x152, Delete: 0x153, Minus: 12, Equal: 13, BracketLeft: 26, BracketRight: 27, Semicolon: 39, Quote: 40, Backquote: 41, Backslash: 43, Comma: 51, Period: 52, Slash: 53 };
for (const [letters, start] of [["QWERTYUIOP", 16], ["ASDFGHJKL", 30], ["ZXCVBNM", 44]] as const) [...letters].forEach((letter, i) => { codes[`Key${letter}`] = start + i; });
[..."1234567890"].forEach((digit, i) => { codes[`Digit${digit}`] = i + 2; });
for (let i = 1; i <= 12; i++) codes[`F${i}`] = i <= 10 ? i + 58 : i + 76;
const keysyms: Record<string, number> = { NumLock: 0xff7f, ScrollLock: 0xff14, Backspace: 0xff08, Tab: 0xff09, Enter: 0xff0d, Escape: 0xff1b, Delete: 0xffff, Home: 0xff50, ArrowLeft: 0xff51, ArrowUp: 0xff52, ArrowRight: 0xff53, ArrowDown: 0xff54, PageUp: 0xff55, PageDown: 0xff56, End: 0xff57, Insert: 0xff63, Shift: 0xffe1, Control: 0xffe3, CapsLock: 0xffe5, Meta: 0xffeb, Alt: 0xffe9 };
export function reservedDesktopKey(event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey">) { return (event.metaKey || event.ctrlKey) && /^Digit[1-9]$/.test(event.code); }
export function desktopKey(event: Pick<KeyboardEvent, "code" | "key" | "location">, down: boolean): DesktopInputEvent | null {
  const scanCode = codes[event.code];
  if (scanCode === undefined) return null;
  let keysym = keysyms[event.key] ?? (/^F\d+$/.test(event.key) ? 0xffbd + Number(event.key.slice(1)) : 0);
  if ([...event.key].length === 1) { const point = event.key.codePointAt(0)!; keysym = point <= 255 ? point : 0x1000000 + point; }
  if (event.location === 2 && ["Shift", "Control", "Meta", "Alt"].includes(event.key)) keysym += 1;
  return { kind: "key", scanCode, keysym, down };
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
export function decodeDesktopFrame(buffer: ArrayBuffer, after: bigint) {
  if (!buffer.byteLength) return null;
  if (buffer.byteLength < 40) throw new Error("invalidFrame");
  const view = new DataView(buffer), sequence = view.getBigUint64(0, true), base = view.getBigUint64(8, true);
  const width = view.getUint32(16, true), height = view.getUint32(20, true);
  const x = view.getUint32(24, true), y = view.getUint32(28, true);
  const rectWidth = view.getUint32(32, true), rectHeight = view.getUint32(36, true);
  if (sequence <= after) return null;
  if (!width || !height || width > 8192 || height > 8192 || width * height > 16_777_216
    || !rectWidth || !rectHeight || x + rectWidth > width || y + rectHeight > height
    || buffer.byteLength !== 40 + rectWidth * rectHeight * 4
    || (base === 0n && (x !== 0 || y !== 0 || rectWidth !== width || rectHeight !== height))
    || base >= sequence) throw new Error("invalidFrame");
  return { sequence, base, width, height, x, y, rectWidth, rectHeight, rgba: new Uint8ClampedArray(buffer, 40) };
}
