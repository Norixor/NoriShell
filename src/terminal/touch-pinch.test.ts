import { describe, expect, it } from "vitest";

import {
  PINCH_MIN_SPAN_PX,
  PinchFontSizeTracker,
  clampTerminalFontSize,
  nextTouchGesturePhase,
  touchSpan,
} from "./touch-pinch";

describe("touch gesture phase", () => {
  it("turns a second finger into a pinch, also during a scroll", () => {
    expect(nextTouchGesturePhase("idle", 1)).toBe("single");
    expect(nextTouchGesturePhase("single", 1)).toBe("single");
    expect(nextTouchGesturePhase("single", 2)).toBe("pinch");
    expect(nextTouchGesturePhase("idle", 2)).toBe("pinch");
    expect(nextTouchGesturePhase("pinch", 3)).toBe("pinch");
  });

  it("ignores the leftover finger of a pinch until every finger is up", () => {
    expect(nextTouchGesturePhase("pinch", 1)).toBe("settling");
    expect(nextTouchGesturePhase("settling", 1)).toBe("settling");
    expect(nextTouchGesturePhase("settling", 2)).toBe("pinch");
    expect(nextTouchGesturePhase("settling", 0)).toBe("idle");
    expect(nextTouchGesturePhase("pinch", 0)).toBe("idle");
    expect(nextTouchGesturePhase("single", 0)).toBe("idle");
  });
});

describe("pinch font size", () => {
  it("measures the finger span", () => {
    expect(touchSpan({ clientX: 0, clientY: 0 }, { clientX: 30, clientY: 40 })).toBe(50);
  });

  it("clamps to the shared integer range", () => {
    expect(clampTerminalFontSize(4)).toBe(10);
    expect(clampTerminalFontSize(40)).toBe(28);
    expect(clampTerminalFontSize(14)).toBe(14);
  });

  it("scales proportionally and rounds to whole pixels", () => {
    const tracker = new PinchFontSizeTracker();
    tracker.begin(200, 11);
    expect(tracker.update(200)).toBeNull();
    // 11 * 1.05 = 11.55: inside the hysteresis band, no change yet.
    expect(tracker.update(210)).toBeNull();
    // 11 * 1.1 = 12.1: one step.
    expect(tracker.update(220)).toBe(12);
    expect(tracker.size).toBe(12);
    // 11 * 2 = 22.
    expect(tracker.update(400)).toBe(22);
    // Back to the starting span returns to the starting size.
    expect(tracker.update(200)).toBe(11);
  });

  it("does not flicker around a half-step boundary", () => {
    const tracker = new PinchFontSizeTracker();
    tracker.begin(200, 12);
    // 12.5 and 12.6 are not past 12 + 0.65.
    expect(tracker.update(200 * (12.5 / 12))).toBeNull();
    expect(tracker.update(200 * (12.6 / 12))).toBeNull();
    expect(tracker.update(200 * (12.7 / 12))).toBe(13);
    // Coming back down needs to pass 13 - 0.65 as well.
    expect(tracker.update(200 * (12.4 / 12))).toBeNull();
    expect(tracker.update(200 * (12.3 / 12))).toBe(12);
  });

  it("stops at the minimum and maximum", () => {
    const tracker = new PinchFontSizeTracker();
    tracker.begin(300, 11);
    expect(tracker.update(30)).toBe(10);
    expect(tracker.update(20)).toBeNull();
    expect(tracker.update(3000)).toBe(28);
    expect(tracker.update(6000)).toBeNull();
  });

  it("re-anchors from the shown size and guards tiny or invalid spans", () => {
    const tracker = new PinchFontSizeTracker();
    tracker.begin(1, 14);
    // The base span is raised to the minimum, so a near-zero start cannot explode the scale.
    expect(tracker.update(PINCH_MIN_SPAN_PX)).toBeNull();
    expect(tracker.update(Number.NaN)).toBeNull();
    tracker.begin(100, 16.4);
    expect(tracker.size).toBe(16);
    expect(tracker.update(100 * (17 / 16))).toBe(17);
  });
});
