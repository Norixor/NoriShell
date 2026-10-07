/**
 * Cell math for touch selection on phones. xterm only selects with a mouse, so
 * the phone view maps a long-press to buffer cells itself and selects through
 * xterm's public `select(column, row, length)` API. Everything here is pure so
 * it can be tested without a renderer.
 */
export interface SelectionCellLine {
  readonly length: number;
  getCell(column: number): { getChars(): string; getWidth(): number } | undefined;
}

/** A selection as linear cell offsets (`row * cols + column`), both ends inclusive. */
export interface LinearCellRange {
  start: number;
  end: number;
}

function isWordCell(line: SelectionCellLine, column: number, separators: string) {
  const cell = line.getCell(column);
  if (!cell) return false;
  // The second cell of a wide character carries no text but belongs to it.
  if (cell.getWidth() === 0) return true;
  const chars = cell.getChars();
  return chars !== "" && !/^\s+$/u.test(chars) && !separators.includes(chars);
}

/**
 * The word under `column`, as inclusive cell columns, or null when the cell is
 * blank or a separator. Separators follow xterm's `wordSeparator` option so a
 * long-press selects the same unit a desktop double-click does.
 */
export function wordColumnsAt(line: SelectionCellLine, column: number, separators: string): { start: number; end: number } | null {
  if (column < 0 || column >= line.length) return null;
  let anchor = column;
  while (anchor > 0 && line.getCell(anchor)?.getWidth() === 0) anchor -= 1;
  if (!isWordCell(line, anchor, separators)) return null;
  let start = anchor;
  let end = anchor;
  while (start > 0 && isWordCell(line, start - 1, separators)) start -= 1;
  while (end + 1 < line.length && isWordCell(line, end + 1, separators)) end += 1;
  return { start, end };
}

/** Grows an anchored range so it also covers `focus` (a linear cell offset). */
export function extendRange(anchor: LinearCellRange, focus: number): LinearCellRange {
  return { start: Math.min(anchor.start, focus), end: Math.max(anchor.end, focus) };
}

/** Arguments for xterm's `select(column, row, length)` covering a linear range. */
export function xtermSelectArgs(range: LinearCellRange, cols: number): { column: number; row: number; length: number } {
  const start = Math.max(0, Math.min(range.start, range.end));
  const end = Math.max(range.start, range.end);
  return { column: start % cols, row: Math.floor(start / cols), length: end - start + 1 };
}
