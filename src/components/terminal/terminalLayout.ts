export type TerminalSplitDirection = "horizontal" | "vertical";

export interface TerminalPaneNode {
  kind: "pane";
  paneId: string;
  terminalId: string;
}

export interface TerminalSplitNode {
  kind: "split";
  splitId: string;
  direction: TerminalSplitDirection;
  ratio: number;
  first: TerminalLayoutNode;
  second: TerminalLayoutNode;
}

export type TerminalLayoutNode = TerminalPaneNode | TerminalSplitNode;

const TERMINAL_LAYOUT_PARSE_MAX_DEPTH = 64;
const TERMINAL_LAYOUT_PARSE_MAX_NODES = 4096;
const TERMINAL_LAYOUT_ID_MAX_LENGTH = 128;

export function createTerminalPane(paneId: string, terminalId = ""): TerminalPaneNode {
  return { kind: "pane", paneId, terminalId };
}

export function countTerminalPanes(node: TerminalLayoutNode): number {
  if (node.kind === "pane") return 1;
  return countTerminalPanes(node.first) + countTerminalPanes(node.second);
}

export function terminalLayoutDepth(node: TerminalLayoutNode): number {
  if (node.kind === "pane") return 0;
  return 1 + Math.max(terminalLayoutDepth(node.first), terminalLayoutDepth(node.second));
}

export interface TerminalLayoutMinimumSpan {
  widthUnits: number;
  heightUnits: number;
}

export function terminalLayoutMinimumSpan(node: TerminalLayoutNode): TerminalLayoutMinimumSpan {
  return terminalLayoutMinimumSpanWithPendingSplit(node, null, null);
}

export function terminalLayoutMinimumSpanAfterSplit(
  node: TerminalLayoutNode,
  paneId: string,
  direction: TerminalSplitDirection,
): TerminalLayoutMinimumSpan {
  return terminalLayoutMinimumSpanWithPendingSplit(node, paneId, direction);
}

export function findTerminalPane(
  node: TerminalLayoutNode,
  paneId: string,
): TerminalPaneNode | null {
  if (node.kind === "pane") return node.paneId === paneId ? node : null;
  return findTerminalPane(node.first, paneId) ?? findTerminalPane(node.second, paneId);
}

export function terminalPaneDepth(
  node: TerminalLayoutNode,
  paneId: string,
  depth = 0,
): number {
  if (node.kind === "pane") return node.paneId === paneId ? depth : Number.POSITIVE_INFINITY;
  return Math.min(
    terminalPaneDepth(node.first, paneId, depth + 1),
    terminalPaneDepth(node.second, paneId, depth + 1),
  );
}

export function splitTerminalPane(
  node: TerminalLayoutNode,
  paneId: string,
  direction: TerminalSplitDirection,
  newPaneId: string,
  splitId: string,
): TerminalLayoutNode {
  const result = splitNode(node, paneId, direction, newPaneId, splitId);
  return result.changed ? result.node : node;
}

function splitNode(
  node: TerminalLayoutNode,
  paneId: string,
  direction: TerminalSplitDirection,
  newPaneId: string,
  splitId: string,
): { node: TerminalLayoutNode; changed: boolean } {
  if (node.kind === "pane") {
    if (node.paneId !== paneId) {
      return { node, changed: false };
    }
    return {
      changed: true,
      node: {
        kind: "split",
        splitId,
        direction,
        ratio: 0.5,
        first: node,
        second: createTerminalPane(newPaneId),
      },
    };
  }

  const first = splitNode(node.first, paneId, direction, newPaneId, splitId);
  if (first.changed) {
    const updated = { ...node, first: first.node };
    return { node: balanceSplitRatio(updated), changed: true };
  }
  const second = splitNode(node.second, paneId, direction, newPaneId, splitId);
  if (second.changed) {
    const updated = { ...node, second: second.node };
    return { node: balanceSplitRatio(updated), changed: true };
  }
  return { node, changed: false };
}

export function closeTerminalPane(
  node: TerminalLayoutNode,
  paneId: string,
): { node: TerminalLayoutNode; nextActivePaneId: string } {
  if (node.kind === "pane") {
    return { node, nextActivePaneId: node.paneId };
  }

  if (node.first.kind === "pane" && node.first.paneId === paneId) {
    return { node: node.second, nextActivePaneId: firstPaneId(node.second) };
  }
  if (node.second.kind === "pane" && node.second.paneId === paneId) {
    return { node: node.first, nextActivePaneId: lastPaneId(node.first) };
  }

  if (findTerminalPane(node.first, paneId)) {
    const closed = closeTerminalPane(node.first, paneId);
    return { node: { ...node, first: closed.node }, nextActivePaneId: closed.nextActivePaneId };
  }
  if (findTerminalPane(node.second, paneId)) {
    const closed = closeTerminalPane(node.second, paneId);
    return { node: { ...node, second: closed.node }, nextActivePaneId: closed.nextActivePaneId };
  }
  return { node, nextActivePaneId: firstPaneId(node) };
}

export function setTerminalForPane(
  node: TerminalLayoutNode,
  paneId: string,
  terminalId: string,
): TerminalLayoutNode {
  if (node.kind === "pane") {
    return node.paneId === paneId ? { ...node, terminalId } : node;
  }
  return {
    ...node,
    first: setTerminalForPane(node.first, paneId, terminalId),
    second: setTerminalForPane(node.second, paneId, terminalId),
  };
}

export function setTerminalSplitRatio(
  node: TerminalLayoutNode,
  splitId: string,
  ratio: number,
): TerminalLayoutNode {
  if (node.kind === "pane") return node;
  if (node.splitId === splitId) {
    const safeRatio = Number.isFinite(ratio) ? ratio : node.ratio;
    return {
      ...node,
      ratio: Math.min(1 - Number.EPSILON, Math.max(Number.EPSILON, safeRatio)),
    };
  }
  return {
    ...node,
    first: setTerminalSplitRatio(node.first, splitId, ratio),
    second: setTerminalSplitRatio(node.second, splitId, ratio),
  };
}

/**
 * Validates an untrusted persisted layout before it is mounted. Corrupt, oversized, duplicate,
 * or over-deep trees fail as a unit so restore cannot produce unreachable Panes.
 */
export function parseTerminalLayout(value: unknown): TerminalLayoutNode | null {
  const paneIds = new Set<string>();
  const splitIds = new Set<string>();
  const budget = { remainingNodes: TERMINAL_LAYOUT_PARSE_MAX_NODES };
  const parsed = parseTerminalLayoutNode(value, 0, paneIds, splitIds, budget);
  if (!parsed || paneIds.size === 0) return null;
  return parsed;
}

function parseTerminalLayoutNode(
  value: unknown,
  depth: number,
  paneIds: Set<string>,
  splitIds: Set<string>,
  budget: { remainingNodes: number },
): TerminalLayoutNode | null {
  budget.remainingNodes -= 1;
  if (budget.remainingNodes < 0 || !isRecord(value)) return null;
  if (value.kind === "pane") {
    if (!isLayoutId(value.paneId) || typeof value.terminalId !== "string"
      || value.terminalId.length > TERMINAL_LAYOUT_ID_MAX_LENGTH
      || paneIds.has(value.paneId)) return null;
    paneIds.add(value.paneId);
    return { kind: "pane", paneId: value.paneId, terminalId: value.terminalId };
  }
  if (value.kind !== "split" || depth >= TERMINAL_LAYOUT_PARSE_MAX_DEPTH
    || !isLayoutId(value.splitId) || splitIds.has(value.splitId)
    || (value.direction !== "horizontal" && value.direction !== "vertical")
    || typeof value.ratio !== "number" || !Number.isFinite(value.ratio)
    || value.ratio <= 0
    || value.ratio >= 1) return null;

  splitIds.add(value.splitId);
  const first = parseTerminalLayoutNode(value.first, depth + 1, paneIds, splitIds, budget);
  const second = parseTerminalLayoutNode(value.second, depth + 1, paneIds, splitIds, budget);
  if (!first || !second) return null;
  return {
    kind: "split",
    splitId: value.splitId,
    direction: value.direction,
    ratio: value.ratio,
    first,
    second,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function terminalLayoutMinimumSpanWithPendingSplit(
  node: TerminalLayoutNode,
  paneId: string | null,
  direction: TerminalSplitDirection | null,
): TerminalLayoutMinimumSpan {
  if (node.kind === "pane") {
    if (node.paneId !== paneId || direction === null) {
      return { widthUnits: 1, heightUnits: 1 };
    }
    return direction === "horizontal"
      ? { widthUnits: 2, heightUnits: 1 }
      : { widthUnits: 1, heightUnits: 2 };
  }

  const first = terminalLayoutMinimumSpanWithPendingSplit(node.first, paneId, direction);
  const second = terminalLayoutMinimumSpanWithPendingSplit(node.second, paneId, direction);
  return node.direction === "horizontal"
    ? {
        widthUnits: first.widthUnits + second.widthUnits,
        heightUnits: Math.max(first.heightUnits, second.heightUnits),
      }
    : {
        widthUnits: Math.max(first.widthUnits, second.widthUnits),
        heightUnits: first.heightUnits + second.heightUnits,
      };
}

function balanceSplitRatio(node: TerminalSplitNode): TerminalSplitNode {
  const first = terminalLayoutMinimumSpan(node.first);
  const second = terminalLayoutMinimumSpan(node.second);
  const firstUnits = node.direction === "horizontal" ? first.widthUnits : first.heightUnits;
  const secondUnits = node.direction === "horizontal" ? second.widthUnits : second.heightUnits;
  return { ...node, ratio: firstUnits / (firstUnits + secondUnits) };
}

function isLayoutId(value: unknown): value is string {
  return typeof value === "string"
    && value.length > 0
    && value.length <= TERMINAL_LAYOUT_ID_MAX_LENGTH
    && !/\p{C}/u.test(value);
}

function firstPaneId(node: TerminalLayoutNode): string {
  return node.kind === "pane" ? node.paneId : firstPaneId(node.first);
}

function lastPaneId(node: TerminalLayoutNode): string {
  return node.kind === "pane" ? node.paneId : lastPaneId(node.second);
}
