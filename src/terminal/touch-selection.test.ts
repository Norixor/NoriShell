import { describe, expect, it } from "vitest";

import { extendRange, wordColumnsAt, xtermSelectArgs, type SelectionCellLine } from "./touch-selection";

function line(cells: Array<[string, number]>): SelectionCellLine {
  return {
    length: cells.length,
    getCell: (column) => {
      const cell = cells[column];
      return cell ? { getChars: () => cell[0], getWidth: () => cell[1] } : undefined;
    },
  };
}

const ascii = (text: string) => line(Array.from(text).map((character) => [character, 1]));

describe("touch selection cell math", () => {
  it("selects the word under the pressed cell using xterm separators", () => {
    const row = ascii("echo hello(world) ");
    expect(wordColumnsAt(row, 6, " ()")).toEqual({ start: 5, end: 9 });
    expect(wordColumnsAt(row, 0, " ()")).toEqual({ start: 0, end: 3 });
    expect(wordColumnsAt(row, 4, " ()")).toBeNull();
    expect(wordColumnsAt(row, 10, " ()")).toBeNull();
  });

  it("keeps both cells of wide characters inside the word", () => {
    const row = line([["你", 2], ["", 0], ["好", 2], ["", 0], [" ", 1], ["a", 1]]);
    expect(wordColumnsAt(row, 1, " ")).toEqual({ start: 0, end: 3 });
    expect(wordColumnsAt(row, 3, " ")).toEqual({ start: 0, end: 3 });
  });

  it("extends from the anchored word in either direction and maps to xterm.select arguments", () => {
    const anchor = { start: 85, end: 89 };
    expect(extendRange(anchor, 82)).toEqual({ start: 82, end: 89 });
    expect(extendRange(anchor, 170)).toEqual({ start: 85, end: 170 });
    expect(extendRange(anchor, 87)).toEqual(anchor);
    expect(xtermSelectArgs({ start: 85, end: 170 }, 80)).toEqual({ column: 5, row: 1, length: 86 });
  });
});
