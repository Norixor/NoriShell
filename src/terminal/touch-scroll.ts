/**
 * One-finger scrolling for the phone terminal. xterm 6 scrolls through its own
 * wheel-driven scrollable element and never registers a touch gesture target, so a
 * drag on the terminal has nothing native to pan. The view turns vertical drags into
 * whole-line steps itself; this module holds the pure gesture math so it can be
 * tested without a renderer.
 *
 * Sign convention: a positive line count scrolls toward newer output (like a wheel
 * turned down), which is what a finger moving up the screen asks for.
 */

/** Movement (CSS px) before a touch counts as a drag instead of a tap or long-press. */
export const TOUCH_SCROLL_SLOP_PX = 10;

/** Release speed (px/ms) below which a drag ends without inertia. */
export const TOUCH_FLING_MIN_VELOCITY = 0.3;

/** Speed (px/ms) at which a running fling stops. */
export const TOUCH_FLING_STOP_VELOCITY = 0.02;

/** Exponential decay time constant of a fling, in ms. */
export const TOUCH_FLING_TIME_CONSTANT_MS = 325;

/** Upper bound for the release speed, so one flick cannot jump the whole scrollback. */
export const TOUCH_FLING_MAX_VELOCITY = 8;

/** Only samples this recent take part in the release-velocity estimate. */
const VELOCITY_WINDOW_MS = 100;

export type TouchDragAxis = "pending" | "vertical" | "horizontal";

/**
 * Decides once per touch whether the finger is scrolling. Until the slop is crossed
 * the touch may still become a tap or a long-press; after that the dominant axis is
 * locked so a slightly diagonal drag keeps scrolling and a sideways one never does.
 */
export function classifyTouchDrag(dx: number, dy: number, slop = TOUCH_SCROLL_SLOP_PX): TouchDragAxis {
  if (Math.hypot(dx, dy) <= slop) return "pending";
  return Math.abs(dy) >= Math.abs(dx) ? "vertical" : "horizontal";
}

/** Converts pixel movement into whole terminal lines, keeping the sub-line remainder. */
export class TouchLineAccumulator {
  private remainder = 0;

  /** `pixels` is positive toward newer output; returns the whole lines to scroll now. */
  add(pixels: number, cellHeight: number): number {
    if (!(cellHeight > 0) || !Number.isFinite(pixels)) return 0;
    this.remainder += pixels / cellHeight;
    const lines = Math.trunc(this.remainder) || 0;
    this.remainder -= lines;
    return lines;
  }

  reset() {
    this.remainder = 0;
  }
}

interface VelocitySample {
  y: number;
  t: number;
}

/**
 * Follows one scrolling finger: the pixel movement since the previous sample and the
 * release velocity from the most recent samples.
 */
export class TouchScrollTracker {
  private samples: VelocitySample[] = [];

  begin(y: number, t: number) {
    this.samples = [{ y, t }];
  }

  /** Pixels toward newer output since the previous sample (finger up = positive). */
  move(y: number, t: number): number {
    const previous = this.samples[this.samples.length - 1];
    this.samples.push({ y, t });
    while (this.samples.length > 2 && t - (this.samples[0]?.t ?? t) > VELOCITY_WINDOW_MS) this.samples.shift();
    return previous ? previous.y - y : 0;
  }

  /** Release velocity in px/ms toward newer output, clamped; 0 when the finger had stopped. */
  releaseVelocity(t: number): number {
    const recent = this.samples.filter((sample) => t - sample.t <= VELOCITY_WINDOW_MS);
    const first = recent[0];
    const last = recent[recent.length - 1];
    if (!first || !last || recent.length < 2) return 0;
    const elapsed = last.t - first.t;
    if (elapsed <= 0) return 0;
    const velocity = (first.y - last.y) / elapsed;
    return Math.max(-TOUCH_FLING_MAX_VELOCITY, Math.min(TOUCH_FLING_MAX_VELOCITY, velocity));
  }
}

/** True when a release at `velocity` (px/ms) should continue as a fling. */
export function shouldFling(velocity: number) {
  return Math.abs(velocity) >= TOUCH_FLING_MIN_VELOCITY;
}

/**
 * Advances a fling by `elapsedMs`: the distance travelled (px, exact integral of the
 * exponential decay) and the remaining velocity. `done` once it is slower than the
 * stop threshold.
 */
export function flingStep(velocity: number, elapsedMs: number): { distance: number; velocity: number; done: boolean } {
  const dt = Math.max(0, elapsedMs);
  const decay = Math.exp(-dt / TOUCH_FLING_TIME_CONSTANT_MS);
  const distance = velocity * TOUCH_FLING_TIME_CONSTANT_MS * (1 - decay);
  const next = velocity * decay;
  return { distance, velocity: next, done: Math.abs(next) < TOUCH_FLING_STOP_VELOCITY };
}
