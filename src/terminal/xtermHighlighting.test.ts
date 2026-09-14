import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Terminal } from "@xterm/xterm";
import { createTerminalHighlighter } from "./xtermHighlighting";

class FakeWorker {
  static instances: FakeWorker[] = [];
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  postMessage = vi.fn();
  terminate = vi.fn();
  constructor() { FakeWorker.instances.push(this); }
  reply(matches: unknown[] = [], failed = false) { this.onmessage?.({ data: { requestId: this.postMessage.mock.calls.at(-1)?.[0].requestId, matches, failed } } as MessageEvent); }
}
function fixture() {
  const callbacks: (() => void)[] = [];
  const disposals: ReturnType<typeof vi.fn>[] = [];
  const on = (callback: () => void) => { callbacks.push(callback); const dispose = vi.fn(); disposals.push(dispose); return { dispose }; };
  const terminal = {
    rows: 1, cols: 5,
    buffer: { active: { type: "normal", viewportY: 0, length: 1, baseY: 0, cursorY: 0, getLine: () => ({ isWrapped: false, getCell: (index: number) => ({ getChars: () => "ERROR"[index], getWidth: () => 1 }) }) }, onBufferChange: on },
    onWriteParsed: on, onScroll: on, onResize: on,
    registerMarker: vi.fn(() => ({ dispose: vi.fn() })),
    registerDecoration: vi.fn(() => ({ dispose: vi.fn() })),
  };
  return { terminal, callbacks, disposals };
}
const config = { enabled: true, rules: [{ id: "error", label: "Error", pattern: "ERROR", mode: "literal" as const, caseSensitive: true, foreground: "#ff0000", background: "#ffffff", enabled: true }] };
beforeEach(() => { vi.useFakeTimers(); FakeWorker.instances = []; vi.stubGlobal("Worker", FakeWorker); });
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });
describe("xterm highlighting worker lifecycle", () => {
  it("terminates a timed-out regex worker once and resumes only after a configuration change", () => {
    const { terminal, callbacks, disposals } = fixture(); const report = vi.fn(); const highlighter = createTerminalHighlighter(terminal as unknown as Terminal, report);
    highlighter.update(config); vi.advanceTimersByTime(80); const first = FakeWorker.instances[0]!;
    vi.advanceTimersByTime(500); expect(first.terminate).toHaveBeenCalledOnce(); expect(report).toHaveBeenLastCalledWith(true);
    callbacks[0]!(); vi.advanceTimersByTime(2_000); expect(FakeWorker.instances).toHaveLength(1);
    highlighter.update(config); vi.advanceTimersByTime(80); expect(FakeWorker.instances).toHaveLength(2);
    // A queued failure from the terminated worker cannot stop its replacement.
    first.reply([], true); expect(FakeWorker.instances[1]!.terminate).not.toHaveBeenCalled();
    highlighter.dispose(); expect(disposals.every((dispose) => dispose.mock.calls.length === 1)).toBe(true);
  });
  it("discards stale output matches and disposes decorations on invalidation", () => {
    const { terminal, callbacks } = fixture(); const highlighter = createTerminalHighlighter(terminal as unknown as Terminal, vi.fn());
    highlighter.update(config); vi.advanceTimersByTime(80); const worker = FakeWorker.instances[0]!;
    callbacks[0]!(); worker.reply([{ line: 0, ruleId: "error", start: 0, end: 5 }]); expect(terminal.registerDecoration).not.toHaveBeenCalled();
    vi.advanceTimersByTime(80); worker.reply([{ line: 0, ruleId: "error", start: 0, end: 5 }]); expect(terminal.registerDecoration).toHaveBeenCalledOnce();
    const decoration = terminal.registerDecoration.mock.results[0]!.value;
    callbacks[0]!(); expect(decoration.dispose).toHaveBeenCalledOnce(); highlighter.dispose();
  });
});
