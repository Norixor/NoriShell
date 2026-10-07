/**
 * Row geometry for the virtualized Overview grid. Each row has a pitch (its height
 * plus the grid gap); the index answers "where does row N start" and "which row
 * covers offset Y" from prefix sums, so rows may differ in height. The desktop grid
 * feeds uniform pitches (its Cards have a fixed height); the phone feeds measured
 * pitches, because a compact Card grows with its content (open-session row, wrapped
 * text, the system font scale) and must never be clipped to an assumed height.
 */
export interface VirtualRowIndex {
  readonly rowCount: number;
  /** Sum of every pitch: the height the scroll viewport must reserve. */
  readonly totalHeight: number;
  /** Offset of the row's top edge; `row` is clamped to `0..rowCount`. */
  offsetOf(row: number): number;
  /** The row whose extent contains `offset` (the last row for offsets past the end). */
  rowAt(offset: number): number;
}

export function createVirtualRowIndex(pitches: readonly number[]): VirtualRowIndex {
  const offsets = new Array<number>(pitches.length + 1);
  offsets[0] = 0;
  for (let index = 0; index < pitches.length; index += 1) {
    const pitch = pitches[index] ?? 0;
    offsets[index + 1] = (offsets[index] ?? 0) + (Number.isFinite(pitch) && pitch > 0 ? pitch : 0);
  }
  const rowCount = pitches.length;
  const totalHeight = offsets[rowCount] ?? 0;
  return {
    rowCount,
    totalHeight,
    offsetOf(row) {
      return offsets[Math.max(0, Math.min(rowCount, Math.floor(row)))] ?? 0;
    },
    rowAt(offset) {
      if (rowCount === 0 || !(offset > 0)) return 0;
      let low = 0;
      let high = rowCount - 1;
      while (low < high) {
        const middle = (low + high + 1) >> 1;
        if ((offsets[middle] ?? 0) <= offset) low = middle;
        else high = middle - 1;
      }
      return low;
    },
  };
}

/** The pitches of a grid whose rows all share one height. */
export function uniformRowPitches(rowCount: number, pitch: number): number[] {
  return new Array<number>(Math.max(0, rowCount)).fill(pitch);
}
