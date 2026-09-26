import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Terminal } from "@xterm/xterm";
import { createTerminalHighlighter } from "./xtermHighlighting";

class FakeWorker {
  static instances: FakeWorker[] = [];
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessageerror: (() => void) | null = null;
  postMessage = vi.fn();
  terminate = vi.fn();
  constructor() { FakeWorker.instances.push(this); }
  reply(matches: unknown[] = [], failed = false) { this.onmessage?.({ data: { requestId: this.postMessage.mock.calls.at(-1)?.[0].requestId, matches, failed } } as MessageEvent); }
}
function fixture() {
  const callbacks: (() => void)[] = [];
  const disposals: ReturnType<typeof vi.fn>[] = [];
  const on = (callback: () => void) => { callbacks.push(callback); const dispose = vi.fn(); disposals.push(dispose); return { dispose }; };
  let text = "ERROR";
  const terminal = {
    rows: 1, cols: 5,
    buffer: { active: { type: "normal", viewportY: 0, length: 1, baseY: 0, cursorY: 0, getLine: () => ({ isWrapped: false, getCell: (index: number) => ({ getChars: () => text[index], getWidth: () => 1 }) }) }, onBufferChange: on },
    onWriteParsed: on, onScroll: on, onResize: on,
    registerMarker: vi.fn(() => ({ line: 0, isDisposed: false, dispose: vi.fn() })),
    registerDecoration: vi.fn(() => ({ dispose: vi.fn() })),
  };
  return { terminal, callbacks, disposals, setText(value: string) { text = value; } };
}
const config = { enabled: true, rules: [{ id: "error", label: "Error", pattern: "ERROR", mode: "literal" as const, caseSensitive: true, foreground: "#ff0000", background: "#ffffff", enabled: true }] };
beforeEach(() => { vi.useFakeTimers(); FakeWorker.instances = []; vi.stubGlobal("Worker", FakeWorker); });
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });
describe("xterm highlighting worker lifecycle", () => {
  it("terminates a timed-out regex worker once and resumes only after a configuration change", () => {
    const { terminal, callbacks, disposals } = fixture(); const report = vi.fn(); const highlighter = createTerminalHighlighter(terminal as unknown as Terminal, report);
    highlighter.update(config); vi.advanceTimersByTime(80); const first = FakeWorker.instances[0]!;
    vi.advanceTimersByTime(500); expect(first.terminate).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1_500); expect(first.terminate).toHaveBeenCalledOnce(); expect(report).toHaveBeenLastCalledWith("startup-timeout");
    callbacks[0]!(); vi.advanceTimersByTime(2_000); expect(FakeWorker.instances).toHaveLength(1);
    highlighter.update(config); vi.advanceTimersByTime(80); expect(FakeWorker.instances).toHaveLength(2);
    // A queued failure from the terminated worker cannot stop its replacement.
    first.reply([], true); expect(FakeWorker.instances[1]!.terminate).not.toHaveBeenCalled();
    highlighter.dispose(); expect(disposals.every((dispose) => dispose.mock.calls.length === 1)).toBe(true);
  });
  it("keeps a short timeout after the first Worker reply", () => {
    const { terminal, callbacks } = fixture(); const report = vi.fn();
    const highlighter = createTerminalHighlighter(terminal as unknown as Terminal, report);
    highlighter.update(config); vi.advanceTimersByTime(80);
    const worker = FakeWorker.instances[0]!;
    vi.advanceTimersByTime(600); worker.reply();
    expect(worker.terminate).not.toHaveBeenCalled();
    callbacks[0]!(); vi.advanceTimersByTime(80);
    vi.advanceTimersByTime(500);
    expect(worker.terminate).toHaveBeenCalledOnce();
    expect(report).toHaveBeenLastCalledWith("scan-timeout");
    highlighter.dispose();
  });
  it("reports Worker and rule failures without retaining terminal text", () => {
    const { terminal } = fixture(); const report = vi.fn();
    const highlighter = createTerminalHighlighter(terminal as unknown as Terminal, report);
    highlighter.update(config); vi.advanceTimersByTime(80);
    FakeWorker.instances[0]!.reply([], true);
    expect(report).toHaveBeenLastCalledWith("evaluation-failed");
    highlighter.update(config); expect(report).toHaveBeenLastCalledWith(null);
    vi.advanceTimersByTime(80);
    FakeWorker.instances[1]!.onmessageerror?.();
    expect(report).toHaveBeenLastCalledWith("worker-error");
    highlighter.dispose();
  });
  it("reports Worker construction failure separately from a timeout", () => {
    const { terminal } = fixture(); const report = vi.fn();
    vi.stubGlobal("Worker", class { constructor() { throw new Error("unavailable"); } });
    const highlighter = createTerminalHighlighter(terminal as unknown as Terminal, report);
    highlighter.update(config); vi.advanceTimersByTime(80);
    expect(report).toHaveBeenLastCalledWith("worker-unavailable");
    highlighter.dispose();
  });
  it("classifies a startup error before the first response as unavailable", () => {
    const { terminal } = fixture(); const report = vi.fn();
    const highlighter = createTerminalHighlighter(terminal as unknown as Terminal, report);
    highlighter.update(config); vi.advanceTimersByTime(80);
    FakeWorker.instances[0]!.onerror?.();
    expect(report).toHaveBeenLastCalledWith("worker-unavailable");
    highlighter.dispose();
  });
  it("classifies an error after a successful reply as a runtime failure", () => {
    const { terminal } = fixture(); const report = vi.fn();
    const highlighter = createTerminalHighlighter(terminal as unknown as Terminal, report);
    highlighter.update(config); vi.advanceTimersByTime(80);
    const worker = FakeWorker.instances[0]!;
    worker.reply();
    worker.onerror?.();
    expect(report).toHaveBeenLastCalledWith("worker-error");
    highlighter.dispose();
  });
  it("keeps unchanged highlights across frequent redraws and reconciles changed text", () => {
    const { terminal, callbacks, setText } = fixture(); const highlighter = createTerminalHighlighter(terminal as unknown as Terminal, vi.fn());
    highlighter.update(config); vi.advanceTimersByTime(80); const worker = FakeWorker.instances[0]!;
    worker.reply([{ line: 0, ruleId: "error", start: 0, end: 5 }]); expect(terminal.registerDecoration).toHaveBeenCalledOnce();
    const decoration = terminal.registerDecoration.mock.results[0]!.value;
    for (let index = 0; index < 5; index++) callbacks[0]!();
    expect(decoration.dispose).not.toHaveBeenCalled();
    vi.advanceTimersByTime(80);
    callbacks[0]!(); worker.reply([{ line: 0, ruleId: "error", start: 0, end: 5 }]);
    expect(terminal.registerDecoration).toHaveBeenCalledOnce();
    expect(decoration.dispose).not.toHaveBeenCalled();
    setText("OTHER"); callbacks[0]!(); vi.advanceTimersByTime(80);
    worker.reply([]); expect(decoration.dispose).toHaveBeenCalledOnce();
    highlighter.dispose();
  });
  it("ignores a stale worker match when the underlying line changes", () => {
    const { terminal, callbacks, setText } = fixture(); const highlighter = createTerminalHighlighter(terminal as unknown as Terminal, vi.fn());
    highlighter.update(config); vi.advanceTimersByTime(80); const worker = FakeWorker.instances[0]!;
    setText("OTHER"); callbacks[0]!(); worker.reply([{ line: 0, ruleId: "error", start: 0, end: 5 }]);
    expect(terminal.registerDecoration).not.toHaveBeenCalled();
    highlighter.dispose();
  });
});
