import { matchHighlightLines, type HighlightRule } from "./highlighting";

self.onmessage = (event: MessageEvent<{ requestId: number; lines: string[]; rules: HighlightRule[] }>) => {
  const { requestId, lines, rules } = event.data;
  try {
    self.postMessage({ requestId, matches: matchHighlightLines(lines, rules) });
  } catch {
    self.postMessage({ requestId, matches: [], failed: true });
  }
};
