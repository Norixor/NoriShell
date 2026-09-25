<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

import {
  terminalLayoutMinimumSpan,
  terminalLayoutMinimumSpanAfterSplit,
  terminalLayoutMinimumSpanAfterWorkspaceRightSplit,
  type TerminalLayoutNode,
  type TerminalPaneNode,
} from "./terminalLayout";

defineOptions({ name: "NvxTerminalSplitTree" });

const props = withDefaults(
  defineProps<{
    node: TerminalLayoutNode;
    activePaneId: string;
    separatorLabel: string;
    minimumPaneWidth?: number;
    minimumPaneHeight?: number;
  }>(),
  {
    minimumPaneWidth: 400,
    minimumPaneHeight: 240,
  },
);

const emit = defineEmits<{
  activate: [paneId: string];
  resize: [splitId: string, ratio: number];
}>();

defineSlots<{
  pane(props: {
    pane: TerminalPaneNode;
    canSplitHorizontal: boolean;
    canSplitVertical: boolean;
    canSplitWorkspaceRight: boolean;
  }): unknown;
}>();

interface LayoutLength {
  ratio: number;
  pixels: number;
}

interface LayoutRect {
  x: LayoutLength;
  y: LayoutLength;
  width: LayoutLength;
  height: LayoutLength;
}

interface PanePlacement {
  pane: TerminalPaneNode;
  rect: LayoutRect;
}

interface SeparatorPlacement {
  splitId: string;
  direction: "horizontal" | "vertical";
  ratio: number;
  rect: LayoutRect;
  ownerRect: LayoutRect;
  firstMinimumUnits: number;
  secondMinimumUnits: number;
}

const separatorSize = 5;
const root = ref<HTMLElement | null>(null);
const rootSize = ref({ width: 0, height: 0 });
const placements = computed(() => flattenLayout(props.node));
let sizeObserver: ResizeObserver | null = null;

function length(ratio = 0, pixels = 0): LayoutLength {
  return { ratio, pixels };
}

function add(first: LayoutLength, second: LayoutLength): LayoutLength {
  return length(first.ratio + second.ratio, first.pixels + second.pixels);
}

function subtract(first: LayoutLength, second: LayoutLength): LayoutLength {
  return length(first.ratio - second.ratio, first.pixels - second.pixels);
}

function scale(value: LayoutLength, ratio: number): LayoutLength {
  return length(value.ratio * ratio, value.pixels * ratio);
}

function flattenLayout(node: TerminalLayoutNode) {
  const panes: PanePlacement[] = [];
  const separators: SeparatorPlacement[] = [];
  collectPlacements(
    node,
    {
      x: length(),
      y: length(),
      width: length(1),
      height: length(1),
    },
    panes,
    separators,
  );
  return { panes, separators };
}

function collectPlacements(
  node: TerminalLayoutNode,
  rect: LayoutRect,
  panes: PanePlacement[],
  separators: SeparatorPlacement[],
) {
  if (node.kind === "pane") {
    panes.push({ pane: node, rect });
    return;
  }

  const separator = length(0, separatorSize);
  const halfSeparator = length(0, separatorSize / 2);
  const firstMinimumSpan = terminalLayoutMinimumSpan(node.first);
  const secondMinimumSpan = terminalLayoutMinimumSpan(node.second);
  if (node.direction === "horizontal") {
    const firstWidth = scale(rect.width, node.ratio);
    const secondWidth = scale(rect.width, 1 - node.ratio);
    const separatorX = add(rect.x, firstWidth);
    collectPlacements(node.first, { ...rect, width: firstWidth }, panes, separators);
    separators.push({
      splitId: node.splitId,
      direction: node.direction,
      ratio: node.ratio,
      ownerRect: rect,
      firstMinimumUnits: firstMinimumSpan.widthUnits,
      secondMinimumUnits: secondMinimumSpan.widthUnits,
      rect: {
        x: subtract(separatorX, halfSeparator),
        y: rect.y,
        width: separator,
        height: rect.height,
      },
    });
    collectPlacements(
      node.second,
      {
        x: separatorX,
        y: rect.y,
        width: secondWidth,
        height: rect.height,
      },
      panes,
      separators,
    );
    return;
  }

  const firstHeight = scale(rect.height, node.ratio);
  const secondHeight = scale(rect.height, 1 - node.ratio);
  const separatorY = add(rect.y, firstHeight);
  collectPlacements(node.first, { ...rect, height: firstHeight }, panes, separators);
  separators.push({
    splitId: node.splitId,
    direction: node.direction,
    ratio: node.ratio,
    ownerRect: rect,
    firstMinimumUnits: firstMinimumSpan.heightUnits,
    secondMinimumUnits: secondMinimumSpan.heightUnits,
    rect: {
      x: rect.x,
      y: subtract(separatorY, halfSeparator),
      width: rect.width,
      height: separator,
    },
  });
  collectPlacements(
    node.second,
    {
      x: rect.x,
      y: separatorY,
      width: rect.width,
      height: secondHeight,
    },
    panes,
    separators,
  );
}

function cssLength(value: LayoutLength) {
  if (Math.abs(value.pixels) < 0.001) return `${value.ratio * 100}%`;
  return `calc(${value.ratio * 100}% ${value.pixels < 0 ? "-" : "+"} ${Math.abs(value.pixels)}px)`;
}

function placementStyle(rect: LayoutRect) {
  return {
    left: cssLength(rect.x),
    top: cssLength(rect.y),
    width: cssLength(rect.width),
    height: cssLength(rect.height),
  };
}

function resolvedLength(value: LayoutLength, total: number) {
  return value.ratio * total + value.pixels;
}

function canSplitPlacement(placement: PanePlacement, direction: "horizontal" | "vertical") {
  const total = direction === "horizontal" ? rootSize.value.width : rootSize.value.height;
  if (total <= 0) return true;
  const minimumSpan = terminalLayoutMinimumSpanAfterSplit(
    props.node,
    placement.pane.paneId,
    direction,
  );
  const requiredSize = direction === "horizontal"
    ? minimumSpan.widthUnits * props.minimumPaneWidth
    : minimumSpan.heightUnits * props.minimumPaneHeight;
  return total >= requiredSize;
}

function canSplitWorkspaceRight() {
  if (rootSize.value.width <= 0) return true;
  const minimumSpan = terminalLayoutMinimumSpanAfterWorkspaceRightSplit(props.node);
  return rootSize.value.width >= minimumSpan.widthUnits * props.minimumPaneWidth;
}

function ratioBounds(placement: SeparatorPlacement) {
  const bounds = root.value?.getBoundingClientRect();
  if (!bounds) return { minimum: 0.15, maximum: 0.85 };
  const horizontal = placement.direction === "horizontal";
  const rootLength = horizontal ? bounds.width : bounds.height;
  const ownerSize = resolvedLength(
    horizontal ? placement.ownerRect.width : placement.ownerRect.height,
    rootLength,
  );
  if (ownerSize <= 0) return { minimum: 0.15, maximum: 0.85 };
  const minimumSize = horizontal ? props.minimumPaneWidth : props.minimumPaneHeight;
  const minimum = placement.firstMinimumUnits * minimumSize / ownerSize;
  const maximum = 1 - placement.secondMinimumUnits * minimumSize / ownerSize;
  if (minimum <= maximum) return { minimum, maximum };
  const balanced = placement.firstMinimumUnits
    / (placement.firstMinimumUnits + placement.secondMinimumUnits);
  return { minimum: balanced, maximum: balanced };
}

function updateRatio(
  placement: SeparatorPlacement,
  clientX: number,
  clientY: number,
) {
  if (!root.value) return;
  const bounds = root.value.getBoundingClientRect();
  const horizontal = placement.direction === "horizontal";
  const rootSize = horizontal ? bounds.width : bounds.height;
  const ownerStart = resolvedLength(
    horizontal ? placement.ownerRect.x : placement.ownerRect.y,
    rootSize,
  );
  const ownerSize = resolvedLength(
    horizontal ? placement.ownerRect.width : placement.ownerRect.height,
    rootSize,
  );
  const available = ownerSize;
  if (available <= 0) return;
  const pointer = (horizontal ? clientX - bounds.left : clientY - bounds.top)
    - ownerStart;
  const { minimum, maximum } = ratioBounds(placement);
  emit(
    "resize",
    placement.splitId,
    Math.min(maximum, Math.max(minimum, pointer / available)),
  );
}

function startResize(placement: SeparatorPlacement, event: PointerEvent) {
  const separator = event.currentTarget as HTMLElement;
  separator.setPointerCapture(event.pointerId);
  updateRatio(placement, event.clientX, event.clientY);

  const move = (moveEvent: PointerEvent) =>
    updateRatio(placement, moveEvent.clientX, moveEvent.clientY);
  const finish = () => {
    separator.removeEventListener("pointermove", move);
    separator.removeEventListener("pointerup", finish);
    separator.removeEventListener("pointercancel", finish);
  };
  separator.addEventListener("pointermove", move);
  separator.addEventListener("pointerup", finish);
  separator.addEventListener("pointercancel", finish);
}

function resizeFromKeyboard(placement: SeparatorPlacement, event: KeyboardEvent) {
  const horizontal = placement.direction === "horizontal";
  const decreaseKey = horizontal ? "ArrowLeft" : "ArrowUp";
  const increaseKey = horizontal ? "ArrowRight" : "ArrowDown";
  const { minimum, maximum } = ratioBounds(placement);
  if (event.key === "Home") {
    event.preventDefault();
    emit("resize", placement.splitId, minimum);
  } else if (event.key === "End") {
    event.preventDefault();
    emit("resize", placement.splitId, maximum);
  } else if (event.key === decreaseKey || event.key === increaseKey) {
    event.preventDefault();
    emit(
      "resize",
      placement.splitId,
      Math.min(
        maximum,
        Math.max(minimum, placement.ratio + (event.key === increaseKey ? 0.05 : -0.05)),
      ),
    );
  }
}

onMounted(() => {
  if (!root.value) return;
  const updateSize = () => {
    const bounds = root.value?.getBoundingClientRect();
    if (bounds) rootSize.value = { width: bounds.width, height: bounds.height };
  };
  updateSize();
  if (typeof ResizeObserver !== "undefined") {
    sizeObserver = new ResizeObserver(updateSize);
    sizeObserver.observe(root.value);
  }
});

onBeforeUnmount(() => {
  sizeObserver?.disconnect();
});
</script>

<template>
  <div
    ref="root"
    class="nvx-terminal-split-tree"
  >
    <section
      v-for="placement in placements.panes"
      :key="placement.pane.paneId"
      class="nvx-terminal-split-tree__pane"
      :class="{ 'nvx-terminal-split-tree__pane--active': placement.pane.paneId === activePaneId }"
      :style="placementStyle(placement.rect)"
      :data-pane-id="placement.pane.paneId"
      @pointerdown="$emit('activate', placement.pane.paneId)"
      @focusin="$emit('activate', placement.pane.paneId)"
    >
      <slot
        name="pane"
        :pane="placement.pane"
        :can-split-horizontal="canSplitPlacement(placement, 'horizontal')"
        :can-split-vertical="canSplitPlacement(placement, 'vertical')"
        :can-split-workspace-right="canSplitWorkspaceRight()"
      />
    </section>

    <div
      v-for="placement in placements.separators"
      :key="placement.splitId"
      class="nvx-terminal-split-tree__separator"
      :class="`nvx-terminal-split-tree__separator--${placement.direction}`"
      :style="placementStyle(placement.rect)"
      role="separator"
      tabindex="0"
      :aria-label="separatorLabel"
      :aria-orientation="placement.direction === 'horizontal' ? 'vertical' : 'horizontal'"
      :aria-valuenow="Math.round(placement.ratio * 100)"
      aria-valuemin="15"
      aria-valuemax="85"
      @dblclick="$emit('resize', placement.splitId, 0.5)"
      @keydown="resizeFromKeyboard(placement, $event)"
      @pointerdown.prevent="startResize(placement, $event)"
    >
      <span aria-hidden="true" />
    </div>
  </div>
</template>

<style scoped>
.nvx-terminal-split-tree {
  position: relative;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--nvx-color-terminal-bg);
}

.nvx-terminal-split-tree__pane {
  position: absolute;
  box-sizing: border-box;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  border: var(--nvx-border-width) solid transparent;
  background: var(--nvx-color-terminal-bg);
}

.nvx-terminal-split-tree__pane--active {
  border-color: var(--nvx-color-terminal-pane-active-border);
}

.nvx-terminal-split-tree__separator {
  position: absolute;
  z-index: 1;
  display: grid;
  place-items: center;
  min-width: 0;
  min-height: 0;
  background: transparent;
  touch-action: none;
}

.nvx-terminal-split-tree__separator::before {
  position: absolute;
  content: "";
  background: var(--nvx-color-border);
}

.nvx-terminal-split-tree__separator--horizontal {
  cursor: col-resize;
}

.nvx-terminal-split-tree__separator--horizontal::before {
  width: var(--nvx-border-width);
  height: 100%;
}

.nvx-terminal-split-tree__separator--vertical {
  cursor: row-resize;
}

.nvx-terminal-split-tree__separator--vertical::before {
  width: 100%;
  height: var(--nvx-border-width);
}

.nvx-terminal-split-tree__separator span {
  width: 3px;
  height: 28px;
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-text-tertiary);
  opacity: 0;
  transition: opacity var(--nvx-motion-fast);
}

.nvx-terminal-split-tree__separator--vertical span {
  width: 28px;
  height: 3px;
}

.nvx-terminal-split-tree__separator:hover span,
.nvx-terminal-split-tree__separator:focus-visible span {
  opacity: 1;
}

.nvx-terminal-split-tree__separator:hover::before,
.nvx-terminal-split-tree__separator:focus-visible::before {
  background: var(--nvx-color-border-strong);
}

.nvx-terminal-split-tree__separator:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: -2px;
}

@media (prefers-reduced-motion: reduce) {
  .nvx-terminal-split-tree__separator span {
    transition: none;
  }
}
</style>
