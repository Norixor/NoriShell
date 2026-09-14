import { beforeEach, describe, expect, it } from "vitest";
import { placePluginFloatingButtons, readPluginFloatingPreferences } from "./pluginFloatingLayout";

describe("host-owned floating placement", () => {
  beforeEach(() => localStorage.clear());
  it("keeps colliding saved buttons inside the viewport without overlap", () => {
    const preferences = Object.fromEntries(["a", "b", "c", "d"].map((id) => [id, { x: 1, y: 1, hidden: false }]));
    const positions = Object.values(placePluginFloatingButtons(Object.keys(preferences), preferences, 800, 400));
    for (const [index, point] of positions.entries()) {
      expect(point.x).toBeGreaterThanOrEqual(0); expect(point.x + 44).toBeLessThanOrEqual(800);
      expect(point.y).toBeGreaterThanOrEqual(0); expect(point.y + 44).toBeLessThanOrEqual(400);
      expect(positions.slice(index + 1).every((other) => Math.abs(other.x - point.x) >= 52 || Math.abs(other.y - point.y) >= 52)).toBe(true);
    }
  });
  it("adapts remembered ratios after resize and rejects unbounded preferences", () => {
    const positions = placePluginFloatingButtons(["task"], { task: { x: 1, y: 1, hidden: false } }, 240, 200);
    expect(positions.task).toEqual({ x: 184, y: 144 });
    localStorage.setItem("prefs", JSON.stringify({ valid: { x: .5, y: .2, hidden: true }, invalid: { x: 10000, y: 0, hidden: false }, script: "url(javascript:bad)" }));
    expect(readPluginFloatingPreferences("prefs")).toEqual({ valid: { x: .5, y: .2, hidden: true } });
  });
});
