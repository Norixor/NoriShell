import { describe, expect, it } from "vitest";

import { createVirtualRowIndex, uniformRowPitches } from "./overviewVirtualRows";

describe("overviewVirtualRows", () => {
  it("reproduces the uniform grid arithmetic", () => {
    const index = createVirtualRowIndex(uniformRowPitches(40, 116));
    expect(index.rowCount).toBe(40);
    expect(index.totalHeight).toBe(40 * 116);
    expect(index.offsetOf(0)).toBe(0);
    expect(index.offsetOf(3)).toBe(348);
    expect(index.offsetOf(99)).toBe(40 * 116);
    expect(index.rowAt(0)).toBe(0);
    expect(index.rowAt(115)).toBe(0);
    expect(index.rowAt(116)).toBe(1);
    expect(index.rowAt(4_000)).toBe(34);
    expect(index.rowAt(1_000_000)).toBe(39);
  });

  it("places rows by their measured heights so a taller Card never overlaps the next row", () => {
    // Row 1 holds a Card with an open-session row (taller), the others the default pitch.
    const index = createVirtualRowIndex([116, 140, 116, 116]);
    expect(index.totalHeight).toBe(488);
    expect(index.offsetOf(1)).toBe(116);
    expect(index.offsetOf(2)).toBe(256);
    expect(index.rowAt(255)).toBe(1);
    expect(index.rowAt(256)).toBe(2);
  });

  it("tolerates empty, negative and non-finite pitches", () => {
    const empty = createVirtualRowIndex([]);
    expect(empty.totalHeight).toBe(0);
    expect(empty.rowAt(50)).toBe(0);
    expect(empty.offsetOf(2)).toBe(0);
    const odd = createVirtualRowIndex([116, -5, Number.NaN, 116]);
    expect(odd.totalHeight).toBe(232);
    expect(odd.offsetOf(3)).toBe(116);
    expect(odd.rowAt(-10)).toBe(0);
  });
});
