import type { IDisposable, Terminal } from "@xterm/xterm";

import type { HighlightConfiguration, HighlightMatch, HighlightRule } from "./highlighting";

interface CellPosition { row: number; column: number; width: number }
interface LogicalLine { text: string; positions: CellPosition[] }

/** Map UTF-16 match positions back to real cells; wide characters, combining characters, and soft wraps never change output. */
export function visibleHighlightLines(terminal: Terminal): LogicalLine[] {
  const buffer = terminal.buffer.active;
  if (buffer.type !== "normal") return [];
  let start = buffer.viewportY;
  for (let i = 0; i < 16 && start > 0 && buffer.getLine(start)?.isWrapped; i++) start--;
  const end = Math.min(buffer.length, buffer.viewportY + Math.min(terminal.rows, 250) + 16);
  const lines: LogicalLine[] = [];
  for (let row = start; row < end; row++) {
    const physical = buffer.getLine(row);
    if (!physical) break;
    if (row >= buffer.viewportY + terminal.rows && !physical.isWrapped) break;
    let logical = lines.at(-1);
    if (!physical.isWrapped || !logical || logical.text.length >= 4_096) {
      logical = { text: "", positions: [] };
      lines.push(logical);
    }
    for (let column = 0; column < Math.min(terminal.cols, 2_048) && logical.text.length < 4_096; column++) {
      const cell = physical.getCell(column);
      if (!cell || cell.getWidth() === 0) continue;
      const chars = cell.getChars() || " ";
      logical.text += chars;
      for (let i = 0; i < chars.length; i++) logical.positions.push({ row, column, width: cell.getWidth() });
    }
  }
  return lines;
}

export function highlightCellRanges(line: LogicalLine, start: number, end: number) {
  const ranges: { row: number; column: number; width: number }[] = [];
  for (const cell of line.positions.slice(start, end)) {
    const last = ranges.at(-1);
    if (last?.row === cell.row) last.width = Math.max(last.width, cell.column + cell.width - last.column);
    else ranges.push({ ...cell });
  }
  return ranges;
}

export function createTerminalHighlighter(terminal: Terminal, report: (failed: boolean) => void) {
  let config: HighlightConfiguration = { enabled: false, rules: [] };
  let worker: Worker | null = null;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  let scheduled: ReturnType<typeof setTimeout> | undefined;
  let revision = 0;
  let requestId = 0;
  let pending = false;
  let dirty = false;
  let suspended = false;
  let disposed = false;
  let decorations: IDisposable[] = [];

  function clearDecorations() {
    for (const item of decorations) item.dispose();
    decorations = [];
  }
  function stopWorker() {
    clearTimeout(timeout);
    timeout = undefined;
    worker?.terminate();
    worker = null;
    pending = false;
  }
  function suspend() {
    stopWorker();
    suspended = true;
    clearDecorations();
    report(true);
  }
  function run() {
    scheduled = undefined;
    if (disposed || suspended || !config.enabled || !config.rules.some((rule) => rule.enabled)) return;
    if (pending) { dirty = true; return; }
    const lines = visibleHighlightLines(terminal);
    if (!lines.length) { clearDecorations(); return; }
    const version = revision;
    const id = ++requestId;
    const viewport = terminal.buffer.active.viewportY;
    const rules = new Map<string, HighlightRule>(config.rules.map((rule) => [rule.id, rule]));
    try {
      worker ??= new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
      const activeWorker = worker;
      worker.onerror = () => { if (worker === activeWorker) suspend(); };
      worker.onmessage = (event: MessageEvent<{ requestId: number; matches: HighlightMatch[]; failed?: boolean }>) => {
        if (disposed || worker !== activeWorker || event.data.requestId !== id) return;
        clearTimeout(timeout);
        timeout = undefined;
        pending = false;
        if (event.data.failed) { suspend(); return; }
        if (version === revision && viewport === terminal.buffer.active.viewportY && config.enabled) {
          clearDecorations();
          const buffer = terminal.buffer.active;
          const cursorRow = buffer.baseY + buffer.cursorY;
          for (const match of event.data.matches.slice(0, 1_000)) {
            const line = lines[match.line];
            const rule = rules.get(match.ruleId);
            if (!line || !rule) continue;
            for (const range of highlightCellRanges(line, match.start, match.end)) {
              if (range.row < viewport || range.row >= viewport + terminal.rows || range.width < 1) continue;
              const marker = terminal.registerMarker(range.row - cursorRow);
              if (!marker) continue;
              const decoration = terminal.registerDecoration({
                marker, x: range.column, width: Math.min(range.width, terminal.cols - range.column),
                foregroundColor: rule.foreground, backgroundColor: rule.background, layer: "bottom",
              });
              decorations.push(marker);
              if (decoration) decorations.push(decoration);
            }
          }
        }
        if (dirty || version !== revision) { dirty = false; schedule(); }
      };
      pending = true;
      worker.postMessage({ requestId: id, lines: lines.map((line) => line.text), rules: config.rules.map((rule) => ({ ...rule })) });
      // Do not retry the same configuration automatically after a timeout, preventing malicious regular expressions from retaining CPU.
      timeout = setTimeout(suspend, 500);
    } catch { suspend(); }
  }
  function schedule() {
    if (scheduled === undefined && !disposed && !suspended && config.enabled) scheduled = setTimeout(run, 80);
  }
  function invalidate() {
    revision++;
    clearDecorations();
    schedule();
  }
  const listeners = [
    terminal.onWriteParsed(invalidate), terminal.onScroll(invalidate),
    terminal.onResize(invalidate), terminal.buffer.onBufferChange(invalidate),
  ];
  return {
    update(value: HighlightConfiguration) {
      config = { enabled: value.enabled, rules: value.rules.map((rule) => ({ ...rule })) };
      revision++;
      suspended = false;
      report(false);
      stopWorker();
      clearDecorations();
      schedule();
    },
    dispose() {
      disposed = true;
      clearTimeout(scheduled);
      stopWorker();
      clearDecorations();
      listeners.forEach((listener) => listener.dispose());
    },
  };
}
