/**
 * Two-finger pinch on the phone terminal scales the terminal font size. This module
 * holds the pure gesture state and size maths so they can be tested without a
 * renderer; the view applies the preview size, refits xterm and persists the final
 * size through the normal appearance preference.
 */

import { MAX_TERMINAL_FONT_SIZE, MIN_TERMINAL_FONT_SIZE } from "../terminal-theme";

/**
 * Extra fraction of a size step the scaled value must pass beyond the half-way point
 * before the integer size changes, so a pinch held near a boundary does not flicker
 * between two sizes (and refit the terminal) on every small finger tremor.
 */
export const PINCH_SIZE_HYSTERESIS = 0.15;

/** Finger spans below this (CSS px) are too noisy to scale from. */
export const PINCH_MIN_SPAN_PX = 24;

/**
 * The touch gesture the terminal surface is in, derived only from how many fingers
 * are on it:
 * - `idle`: no finger.
 * - `single`: one finger; it may become a tap, long-press or scroll drag.
 * - `pinch`: two or more fingers scale the font.
 * - `settling`: a pinch lost fingers but one remains; it is ignored until every
 *   finger is up, so the leftover finger never scrolls, taps or selects.
 */
export type TouchGesturePhase = "idle" | "single" | "pinch" | "settling";

export function nextTouchGesturePhase(phase: TouchGesturePhase, fingers: number): TouchGesturePhase {
  if (fingers <= 0) return "idle";
  if (fingers >= 2) return "pinch";
  return phase === "pinch" || phase === "settling" ? "settling" : "single";
}

export function clampTerminalFontSize(size: number) {
  return Math.min(MAX_TERMINAL_FONT_SIZE, Math.max(MIN_TERMINAL_FONT_SIZE, size));
}

/** Distance between two touch points in CSS px. */
export function touchSpan(a: { clientX: number; clientY: number }, b: { clientX: number; clientY: number }) {
  return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY);
}

/**
 * Follows one pinch: the font size scales proportionally with the finger span and is
 * rounded to whole pixels, so one step needs roughly a 1/size change in span (about 9%
 * at 11px) — finer at large sizes, never a jump of several sizes for a small move.
 */
export class PinchFontSizeTracker {
  private baseSpan = 0;
  private baseSize = 0;
  private current = 0;

  /** Starts (or re-anchors, when fingers change) from the size currently shown. */
  begin(span: number, size: number) {
    this.baseSpan = Math.max(span, PINCH_MIN_SPAN_PX);
    this.baseSize = clampTerminalFontSize(Math.round(size));
    this.current = this.baseSize;
  }

  get size() {
    return this.current;
  }

  /** Returns the new integer size when it changed, otherwise null. */
  update(span: number): number | null {
    if (!(this.baseSpan > 0) || !Number.isFinite(span)) return null;
    const scaled = this.baseSize * (Math.max(span, PINCH_MIN_SPAN_PX) / this.baseSpan);
    const threshold = 0.5 + PINCH_SIZE_HYSTERESIS;
    if (Math.abs(scaled - this.current) < threshold) return null;
    const next = clampTerminalFontSize(Math.round(scaled));
    if (next === this.current) return null;
    this.current = next;
    return next;
  }
}
