import { describe, expect, it } from "vitest";

import {
  TOUCH_FLING_MAX_VELOCITY,
  TOUCH_FLING_TIME_CONSTANT_MS,
  TOUCH_SCROLL_SLOP_PX,
  TouchLineAccumulator,
  TouchScrollTracker,
  classifyTouchDrag,
  flingStep,
  shouldFling,
} from "./touch-scroll";

describe("classifyTouchDrag", () => {
  it("stays pending inside the slop so taps and long-presses survive finger jitter", () => {
    expect(classifyTouchDrag(0, 0)).toBe("pending");
    expect(classifyTouchDrag(6, 7)).toBe("pending");
    expect(classifyTouchDrag(0, TOUCH_SCROLL_SLOP_PX)).toBe("pending");
  });

  it("locks the dominant axis once the slop is crossed", () => {
    expect(classifyTouchDrag(3, -14)).toBe("vertical");
    expect(classifyTouchDrag(-9, 9)).toBe("vertical");
    expect(classifyTouchDrag(15, 4)).toBe("horizontal");
  });
});

describe("TouchLineAccumulator", () => {
  it("emits whole lines and carries the remainder across moves in both directions", () => {
    const lines = new TouchLineAccumulator();
    expect(lines.add(12, 20)).toBe(0);
    expect(lines.add(12, 20)).toBe(1);
    expect(lines.add(40, 20)).toBe(2);
    // 0.2 line left over; reversing first consumes it.
    expect(lines.add(-10, 20)).toBe(0);
    expect(lines.add(-40, 20)).toBe(-2);
    lines.reset();
    expect(lines.add(19, 20)).toBe(0);
  });

  it("ignores unusable geometry", () => {
    const lines = new TouchLineAccumulator();
    expect(lines.add(100, 0)).toBe(0);
    expect(lines.add(Number.NaN, 20)).toBe(0);
    expect(lines.add(40, 20)).toBe(2);
  });
});

describe("TouchScrollTracker", () => {
  it("reports finger-up movement as positive (toward newer output)", () => {
    const tracker = new TouchScrollTracker();
    tracker.begin(500, 0);
    expect(tracker.move(480, 16)).toBe(20);
    expect(tracker.move(490, 32)).toBe(-10);
  });

  it("estimates the release velocity from recent samples only", () => {
    const tracker = new TouchScrollTracker();
    tracker.begin(800, 0);
    tracker.move(700, 16);
    tracker.move(600, 32);
    tracker.move(500, 48);
    expect(tracker.releaseVelocity(50)).toBeCloseTo(300 / 48, 5);
    // A finger that stopped before lifting carries no inertia.
    expect(tracker.releaseVelocity(400)).toBe(0);
  });

  it("clamps a flick to the maximum fling speed", () => {
    const tracker = new TouchScrollTracker();
    tracker.begin(2000, 0);
    tracker.move(0, 10);
    expect(tracker.releaseVelocity(10)).toBe(TOUCH_FLING_MAX_VELOCITY);
  });
});

describe("fling", () => {
  it("only starts above the minimum release speed", () => {
    expect(shouldFling(0.1)).toBe(false);
    expect(shouldFling(-0.5)).toBe(true);
  });

  it("decays exponentially and the total distance equals velocity × time constant", () => {
    let velocity = 2;
    let total = 0;
    let done = false;
    for (let frame = 0; frame < 1000 && !done; frame += 1) {
      const step = flingStep(velocity, 16);
      total += step.distance;
      expect(Math.abs(step.velocity)).toBeLessThan(Math.abs(velocity));
      velocity = step.velocity;
      done = step.done;
    }
    expect(done).toBe(true);
    expect(total).toBeGreaterThan(2 * TOUCH_FLING_TIME_CONSTANT_MS * 0.98);
    expect(total).toBeLessThanOrEqual(2 * TOUCH_FLING_TIME_CONSTANT_MS);
  });

  it("keeps the direction of a backward fling", () => {
    const step = flingStep(-1, 16);
    expect(step.distance).toBeLessThan(0);
    expect(step.velocity).toBeLessThan(0);
  });
});
