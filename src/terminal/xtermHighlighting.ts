import type { IDisposable, IMarker, Terminal } from "@xterm/xterm";

import type { HighlightConfiguration, HighlightMatch, HighlightRule } from "./highlighting";

interface CellPosition { row: number; column: number; width: number }
interface LogicalLine { text: string; positions: CellPosition[] }
interface HighlightDecoration { marker: IMarker; decoration: IDisposable; column: number; width: number; foreground: string; background: string }
export type HighlightSuspensionReason = "worker-unavailable" | "worker-error" | "evaluation-failed" | "startup-timeout" | "scan-timeout";

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

export function createTerminalHighlighter(terminal: Terminal, report: (reason: HighlightSuspensionReason | null) => void) {
  let config: HighlightConfiguration = { enabled: false, rules: [] };
  let worker: Worker | null = null;
  let workerResponded = false;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  let scheduled: ReturnType<typeof setTimeout> | undefined;
  let revision = 0;
  let requestId = 0;
  let pending = false;
  let dirty = false;
  let suspended = false;
  let disposed = false;
  let decorations: HighlightDecoration[] = [];

  function clearDecorations() {
    for (const item of decorations) { item.decoration.dispose(); item.marker.dispose(); }
    decorations = [];
  }
  function stopWorker() {
    clearTimeout(timeout);
    timeout = undefined;
    worker?.terminate();
    worker = null;
    workerResponded = false;
    pending = false;
  }
  function suspend(reason: HighlightSuspensionReason) {
    stopWorker();
    suspended = true;
    clearDecorations();
    report(reason);
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
    const coldWorker = worker === null;
    try {
      worker ??= new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
    } catch { suspend("worker-unavailable"); return; }
    try {
      const activeWorker = worker;
      worker.onerror = () => { if (worker === activeWorker) suspend(workerResponded ? "worker-error" : "worker-unavailable"); };
      worker.onmessageerror = () => { if (worker === activeWorker) suspend("worker-error"); };
      worker.onmessage = (event: MessageEvent<{ requestId: number; matches: HighlightMatch[]; failed?: boolean }>) => {
        if (disposed || worker !== activeWorker) return;
        workerResponded = true;
        if (event.data.requestId !== id) return;
        clearTimeout(timeout);
        timeout = undefined;
        pending = false;
        if (event.data.failed || !Array.isArray(event.data.matches)) { suspend("evaluation-failed"); return; }
        if (config.enabled && terminal.buffer.active.type === "normal") {
          const buffer = terminal.buffer.active;
          const currentLines = version === revision && viewport === buffer.viewportY ? lines : visibleHighlightLines(terminal);
          const cursorRow = buffer.baseY + buffer.cursorY;
          const previous = new Map<string, HighlightDecoration>();
          for (const item of decorations) {
            if (!item.marker.isDisposed) previous.set(`${item.marker.line}:${item.column}:${item.width}:${item.foreground}:${item.background}`, item);
          }
          const next: HighlightDecoration[] = [];
          const seen = new Set<string>();
          for (const match of event.data.matches.slice(0, 1_000)) {
            const line = lines[match.line];
            const current = currentLines[match.line];
            const rule = rules.get(match.ruleId);
            if (!line || !current || !rule || line.text !== current.text) continue;
            const currentRanges = highlightCellRanges(current, match.start, match.end);
            for (const range of highlightCellRanges(line, match.start, match.end)) {
              if (range.row < buffer.viewportY || range.row >= buffer.viewportY + terminal.rows || range.width < 1) continue;
              if (!currentRanges.some((item) => item.row === range.row && item.column === range.column && item.width === range.width)) continue;
              const width = Math.min(range.width, terminal.cols - range.column);
              if (width < 1) continue;
              const key = `${range.row}:${range.column}:${width}:${rule.foreground}:${rule.background}`;
              if (seen.has(key)) continue;
              seen.add(key);
              const existing = previous.get(key);
              if (existing) { previous.delete(key); next.push(existing); continue; }
              const marker = terminal.registerMarker(range.row - cursorRow);
              if (!marker) continue;
              const decoration = terminal.registerDecoration({
                marker, x: range.column, width,
                foregroundColor: rule.foreground, backgroundColor: rule.background, layer: "bottom",
              });
              if (decoration) next.push({ marker, decoration, column: range.column, width, foreground: rule.foreground, background: rule.background });
              else marker.dispose();
            }
          }
          const retained = new Set(next);
          for (const item of decorations) if (!retained.has(item)) { item.decoration.dispose(); item.marker.dispose(); }
          decorations = next;
        }
        if (dirty || version !== revision) { dirty = false; schedule(); }
      };
      pending = true;
      worker.postMessage({ requestId: id, lines: lines.map((line) => line.text), rules: config.rules.map((rule) => ({ ...rule })) });
      // The first reply includes module Worker startup; later scans keep the shorter regex budget.
      // Do not retry the same configuration automatically after a timeout.
      timeout = setTimeout(() => suspend(coldWorker ? "startup-timeout" : "scan-timeout"), coldWorker ? 2_000 : 500);
    } catch { suspend(coldWorker ? "worker-unavailable" : "worker-error"); }
  }
  function schedule() {
    if (scheduled === undefined && !disposed && !suspended && config.enabled) scheduled = setTimeout(run, 80);
  }
  function invalidate() {
    revision++;
    schedule();
  }
  function resetGeometry() {
    clearDecorations();
    invalidate();
  }
  const listeners = [
    terminal.onWriteParsed(invalidate), terminal.onScroll(invalidate),
    terminal.onResize(resetGeometry), terminal.buffer.onBufferChange(resetGeometry),
  ];
  return {
    update(value: HighlightConfiguration) {
      config = { enabled: value.enabled, rules: value.rules.map((rule) => ({ ...rule })) };
      revision++;
      suspended = false;
      report(null);
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
