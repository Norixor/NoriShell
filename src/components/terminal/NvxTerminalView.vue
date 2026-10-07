<script setup lang="ts">
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Terminal, type IBufferLine, type IDecoration, type IDisposable, type ILink, type ILinkProvider, type IMarker } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { X } from "lucide-vue-next";
import { nextTick, onBeforeUnmount, onMounted, ref, watch, type ComponentPublicInstance } from "vue";
import { useI18n } from "vue-i18n";

import { useUiStore } from "../../stores/ui";
import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";
import { useTipsStore } from "../../stores/tips";
import { detectDesktopPlatform } from "../../platform";
import { terminalFontCssFamily } from "../../terminal-theme";
import { createTerminalHighlighter, type HighlightSuspensionReason } from "../../terminal/xtermHighlighting";
import { TerminalDraftTracker } from "../../terminal/draft";
import { captureTerminalInput } from "../../terminal-input-target";
import { isMobileLayout } from "../../platform-host";
import { useMobileImeVisible } from "../mobile/useMobileLandscape";
import { applyStickyModifiers, stickyAlt, stickyCtrl } from "../../terminal/extra-keys";
import { createMobileImeInput, type MobileImeInputController } from "../../terminal/mobile-ime-input";
import { extendRange, wordColumnsAt, xtermSelectArgs, type LinearCellRange } from "../../terminal/touch-selection";
import {
  TouchLineAccumulator,
  TouchScrollTracker,
  classifyTouchDrag,
  flingStep,
  shouldFling,
  type TouchDragAxis,
} from "../../terminal/touch-scroll";
import {
  PinchFontSizeTracker,
  nextTouchGesturePhase,
  touchSpan,
  type TouchGesturePhase,
} from "../../terminal/touch-pinch";
import { wordSeparatorForDoubleClickSelection } from "../../terminal/interaction-preferences";
import { MacOptionKeyTracker, isPlainBackspace } from "../../terminal/keyboard-compatibility";
import { TerminalBellAudio, type TerminalBellAudioAvailability } from "../../terminal/terminal-bell";
import { geometryForOutput, recordOutputGeometry, type TerminalOutputGeometryMarker } from "../../terminal-output-geometry";
import { findTerminalHttpLinks, openSafeTerminalHttpUrl, safeTerminalHttpUrl, terminalLinkModifierPressed } from "../../terminal/terminal-links";
import { usePopoverMenu } from "../ui/usePopoverMenu";
import NvxTerminalPasteGuard from "./NvxTerminalPasteGuard.vue";
import { NvxIcon } from "../ui";

const props = defineProps<{
  readOnly: boolean;
  reconnectOnInput?: boolean;
  terminalLabel: string;
  gapLabel: string;
  paneId?: string;
  hostId?: string | null;
  ghostSuggestion?: { draft: string; suffix: string } | null;
  initialDimensions?: { rows: number; cols: number };
  initialOutputGeometry?: TerminalOutputGeometryMarker[];
}>();

const emit = defineEmits<{
  input: [value: string, observedCommand?: string];
  reconnectRequest: [];
  resize: [rows: number, cols: number];
  selectionChange: [hasSelection: boolean];
  searchRequest: [];
  draftChange: [draft: string | null];
  bellAttention: [active: boolean];
  acceptSuggestion: [];
}>();

const host = ref<HTMLElement | null>(null);
const viewRoot = ref<HTMLElement | null>(null);
const ui = useUiStore();
const { t } = useI18n();
const preferences = useTerminalPreferencesStore();
const tips = useTipsStore();
const pasteGuard = ref<InstanceType<typeof NvxTerminalPasteGuard> | null>(null);
const highlightFailure = ref<HighlightSuspensionReason | null>(null);
const contextPosition = ref({ left: 0, top: 0 });
const bellFlash = ref(false);
const contextLinkUrl = ref<string | null>(null);
const { rootRef, open: contextMenuOpen, closeMenu: closeContextMenuPopover, toggleMenu: toggleContextMenu, handleMenuKeyDown } = usePopoverMenu();
let highlighter: ReturnType<typeof createTerminalHighlighter> | null = null;
const draftTracker = new TerminalDraftTracker();
let simplePrompt: { line: number; prefix: string } | null = null;
let shellTabCandidate: { prefix: string; typedAfterTab: string } | null = null;
let opaqueInteractive = false;
let alternateActive = false;
let alternateEntryLine: number | null = null;
let awaitingShellAfterAlternate = false;
let blockedLine: number | null = null;
let terminal: Terminal | null = null;
let lastWriteParsed: Promise<void> = Promise.resolve();
let pendingRenderOperations = 0;
let lastOutputSeq = "0";
let replayingOutput = false;
const outputGeometry: TerminalOutputGeometryMarker[] = props.initialOutputGeometry?.map((marker) => ({ ...marker })) ?? [];
let fitAddon: FitAddon | null = null;
let searchAddon: SearchAddon | null = null;
let observer: ResizeObserver | null = null;
let animationFrame = 0;
// The first xterm fit may happen after a new Tab WebView becomes visible,
// before this component can receive the shell's visibility event.
let refreshOnNextFit = true;
let visibleRefreshFrame = 0;
let lastRows = 0;
let lastCols = 0;
let reducedMotionQuery: MediaQueryList | null = null;
let removeReducedMotionListener: (() => void) | null = null;
let selectionMouseGestureActive = false;
let selectionAtMouseDown: string | null = null;
let bellDisposable: IDisposable | null = null;
let ghostDecoration: IDecoration | null = null;
let ghostMarker: IMarker | null = null;
let ghostWriteListener: IDisposable | null = null;
let ghostResizeListener: IDisposable | null = null;
let ghostFrame = 0;
let ghostKey = "";
let linkProviderDisposable: IDisposable | null = null;
let bellFlashTimer: number | null = null;
let lastBellVisualAt = 0;
let lastBellFeedbackAt = 0;
let bellAudioAvailability: TerminalBellAudioAvailability | null = null;
let hoveredLinkUrl: string | null = null;
let bellAttentionActive = false;
let mobileIme: MobileImeInputController | null = null;
let selectionChangeDisposable: IDisposable | null = null;
// Phone touch selection: the pending long-press, and the word it anchored while the same touch drags.
let longPressTimer: ReturnType<typeof setTimeout> | null = null;
let touchOrigin: { identifier: number; x: number; y: number } | null = null;
let touchSelectionAnchor: LinearCellRange | null = null;
let touchSelectedRange: LinearCellRange | null = null;
let discardingImeText = false;
let touchSelectionDragging = false;
// Phone touch scrolling: the locked axis of the current touch, the textarea `inputmode`
// it found (restored when the touch turns out to be a drag), and the running fling.
let touchDragAxis: TouchDragAxis = "pending";
let touchInputModeBefore: string | null = null;
let touchStoppedFling = false;
const touchScrollTracker = new TouchScrollTracker();
const touchScrollLines = new TouchLineAccumulator();
let touchFlingFrame = 0;
let touchFlingVelocity = 0;
let touchFlingLastAt = 0;
// Phone pinch zoom: the gesture phase (by finger count), the two fingers being followed,
// the size shown while pinching (applied at most once per animation frame) and the
// latest persist attempt, so an older failed save cannot undo a newer pinch.
let touchGesturePhase: TouchGesturePhase = "idle";
let pinchFingers: [number, number] | null = null;
const pinchTracker = new PinchFontSizeTracker();
let pinchPreviewSize: number | null = null;
let pinchFrame = 0;
let pinchCommitSeq = 0;
const pinchIndicatorSize = ref<number | null>(null);
// When the last plain tap landed on a still read-only view (input lease pending), so the
// soft keyboard must be opened once the lease arrives. 0 when no such tap is pending.
let mobileKeyboardTapAt = 0;
const mobileSelectionOpen = ref(false);
const mobileSelectionText = ref("");
const optionKeyTracker = new MacOptionKeyTracker();
const bellAudio = new TerminalBellAudio();

function isMacPlatform() { return detectDesktopPlatform() === "macos"; }
const mobileLayout = isMobileLayout();

/**
 * Android soft keyboards must treat the hidden xterm textarea as a terminal, not a
 * sentence field: no auto-capitalisation, no autocorrect, no suggestions, and an
 * Enter key hint rather than a "Go" action. The soft keyboard itself is governed by
 * `inputmode` (see `setMobileInputMode`): it starts as `none`, so the many
 * programmatic focus calls (Pane activation, dialog and menu focus restoration,
 * paste guard) never open the keyboard; only a deliberate tap on the terminal
 * surface switches it to `text`.
 */
function applyMobileTextareaAttributes(textarea: HTMLTextAreaElement | undefined) {
  if (!textarea) return;
  textarea.setAttribute("autocapitalize", "off");
  textarea.setAttribute("autocorrect", "off");
  textarea.setAttribute("autocomplete", "off");
  textarea.setAttribute("spellcheck", "false");
  textarea.setAttribute("enterkeyhint", "enter");
  textarea.setAttribute("inputmode", "none");
}

/**
 * The soft-keyboard state machine of the helper textarea. Android opens the IME
 * whenever an editable element gains focus (also programmatically) and never for
 * `inputmode="none"`. `text` is set only by a touch on the terminal surface, right
 * before xterm focuses the textarea on the synthesized mousedown; `none` returns
 * whenever focus leaves the textarea or the Activity reports the keyboard hidden, so
 * toolbar buttons, popovers, sheets and the Tab switcher can restore focus to the
 * terminal without raising the keyboard. Hardware keyboards are unaffected.
 */
function setMobileInputMode(mode: "text" | "none") {
  const textarea = terminal?.textarea;
  if (textarea && textarea.getAttribute("inputmode") !== mode) textarea.setAttribute("inputmode", mode);
}

/** The Activity's inset fact; `false` also where the signal does not exist (then only taps ever open the keyboard). */
const mobileImeVisible = useMobileImeVisible();

function applicationCursorKeys() {
  return terminal?.modes.applicationCursorKeysMode ?? false;
}

function backspaceSequence() {
  return preferences.resolvedKeyboard(props.hostId).backspaceMode === "bs" ? "\b" : "\x7f";
}

/**
 * Committed soft-keyboard text enters xterm's normal user-input path, so the
 * sticky helper modifiers, draft observation, read-only gate and input ownership
 * all apply exactly as for a physical key.
 */
function deliverMobileText(data: string) {
  if (!terminal || discardingImeText) return;
  if (canRequestReconnect()) {
    // A disconnected Pane treats typing as a recovery gesture and never replays the text.
    emit("reconnectRequest");
    return;
  }
  if (props.readOnly) return;
  const characters = Array.from(data);
  // Printable commits are fed per character so Shell draft observation sees ordinary typing;
  // a pending sticky modifier keeps the whole commit together, as on desktop.
  if (characters.length > 1 && !stickyCtrl.value && !stickyAlt.value && /^[\x20-\x7e]+$/.test(data)) {
    for (const character of characters) terminal.input(character, true);
  } else {
    terminal.input(data, true);
  }
}

/**
 * IME text that is really a paste (a keyboard clipboard chip, drag and drop, any
 * multi-line commit) takes the same guarded path as a paste event: multi-line
 * confirmation, bracketed paste and the captured input ticket.
 */
function deliverMobilePaste(text: string) {
  if (!terminal || discardingImeText || !text) return;
  if (canRequestReconnect()) {
    // Like a paste shortcut on a disconnected Pane: a recovery gesture whose payload is never kept.
    emit("reconnectRequest");
    return;
  }
  if (props.readOnly) return;
  void pasteGuard.value?.requestPaste(text);
}

/**
 * When the soft keyboard opens (most visibly in landscape), Android scrolls the
 * focused helper textarea into view by scrolling `overflow: hidden` ancestors such
 * as `.xterm` and the page container, which pushes the rendered rows above the
 * visible area. The terminal fits itself to the visible viewport instead, so on
 * phones those ancestors are returned to their origin while this terminal owns
 * keyboard focus. Unfocused (e.g. kept-alive) views never touch shared ancestors.
 */
let mobileScrollResetFrame = 0;
function terminalOwnsKeyboardFocus() {
  return Boolean(terminal?.textarea) && document.activeElement === terminal?.textarea;
}

function resetMobileFocusScroll() {
  mobileScrollResetFrame = 0;
  const root = viewRoot.value;
  if (!root || !terminalOwnsKeyboardFocus()) return;
  const elements: HTMLElement[] = [];
  if (terminal?.element) elements.push(terminal.element);
  if (host.value) elements.push(host.value);
  for (let element: HTMLElement | null = root; element && element !== document.body && element !== document.documentElement; element = element.parentElement) {
    elements.push(element);
  }
  for (const element of elements) {
    if (element.scrollTop) element.scrollTop = 0;
    if (element.scrollLeft) element.scrollLeft = 0;
  }
}

function scheduleMobileFocusScrollReset() {
  if (mobileScrollResetFrame) return;
  mobileScrollResetFrame = requestAnimationFrame(resetMobileFocusScroll);
}

function handleMobileAncestorScroll(event: Event) {
  const target = event.target;
  const root = viewRoot.value;
  if (!(target instanceof HTMLElement) || !root) return;
  if (target === terminal?.element || target === host.value || target.contains(root)) scheduleMobileFocusScrollReset();
}

function handleMobileViewportChange() {
  if (!terminalOwnsKeyboardFocus()) return;
  scheduleMobileFocusScrollReset();
  // Fit to the new visible height so the cursor row stays on screen.
  scheduleFit();
}

function terminalScreenGeometry() {
  const screen = terminal?.element?.querySelector<HTMLElement>(".xterm-screen");
  if (!terminal || !screen || !terminal.cols || !terminal.rows) return null;
  const bounds = screen.getBoundingClientRect();
  if (!bounds.width || !bounds.height) return null;
  return { bounds, cellWidth: bounds.width / terminal.cols, cellHeight: bounds.height / terminal.rows };
}

/** Shows IME composition text at the cursor in xterm's own composition element. */
function showCompositionPreview(text: string | null) {
  const view = terminal?.element?.querySelector<HTMLElement>(".composition-view");
  if (!terminal || !view) return;
  if (text === null) {
    view.classList.remove("active");
    view.textContent = "";
    return;
  }
  view.textContent = text;
  view.classList.toggle("active", text.length > 0);
  const geometry = terminalScreenGeometry();
  const buffer = terminal.buffer.active;
  if (!geometry) return;
  const cursorRow = buffer.baseY + buffer.cursorY - buffer.viewportY;
  view.style.left = `${Math.min(buffer.cursorX, terminal.cols - 1) * geometry.cellWidth}px`;
  view.style.top = `${Math.max(0, Math.min(cursorRow, terminal.rows - 1)) * geometry.cellHeight}px`;
  view.style.height = `${geometry.cellHeight}px`;
  view.style.lineHeight = `${geometry.cellHeight}px`;
  view.style.fontFamily = String(terminal.options.fontFamily ?? "");
  view.style.fontSize = `${terminal.options.fontSize ?? ui.terminalFontSize}px`;
}

function linearCellAt(clientX: number, clientY: number) {
  const geometry = terminalScreenGeometry();
  if (!terminal || !geometry) return null;
  const column = Math.max(0, Math.min(terminal.cols - 1, Math.floor((clientX - geometry.bounds.left) / geometry.cellWidth)));
  const viewportRow = Math.max(0, Math.min(terminal.rows - 1, Math.floor((clientY - geometry.bounds.top) / geometry.cellHeight)));
  const row = terminal.buffer.active.viewportY + viewportRow;
  return { column, row, offset: row * terminal.cols + column };
}

function selectLinearRange(range: LinearCellRange) {
  if (!terminal) return;
  const { column, row, length } = xtermSelectArgs(range, terminal.cols);
  terminal.select(column, row, length);
  touchSelectedRange = { start: Math.min(range.start, range.end), end: Math.max(range.start, range.end) };
}

function cancelLongPress() {
  if (longPressTimer !== null) clearTimeout(longPressTimer);
  longPressTimer = null;
}

function closeMobileSelection(clearSelection = true) {
  mobileSelectionOpen.value = false;
  touchSelectionAnchor = null;
  touchSelectedRange = null;
  touchSelectionDragging = false;
  if (clearSelection) terminal?.clearSelection();
}

function handleLongPress(x: number, y: number) {
  longPressTimer = null;
  if (!terminal || isMouseReportingEnabled()) return;
  if (preferences.preferences.interaction.rightClickBehavior === "paste") {
    // The "paste" preference makes a long-press the paste gesture, like a desktop right-click.
    touchOrigin = null;
    closeMobileSelection();
    pasteFromMobileSelection();
    return;
  }
  const cell = linearCellAt(x, y);
  if (!cell) return;
  const line = terminal.buffer.active.getLine(cell.row);
  const word = line ? wordColumnsAt(line, cell.column, String(terminal.options.wordSeparator ?? "")) : null;
  const pressed: LinearCellRange | null = word
    ? { start: cell.row * terminal.cols + word.start, end: cell.row * terminal.cols + word.end }
    : null;
  const existing = touchSelectedRange;
  if (pressed && mobileSelectionOpen.value && existing && terminal.hasSelection()) {
    // A second long-press grows the existing selection to include the pressed word.
    touchSelectionAnchor = { start: Math.min(existing.start, pressed.start), end: Math.max(existing.end, pressed.end) };
  } else {
    touchSelectionAnchor = pressed;
    if (!pressed) {
      terminal.clearSelection();
      touchSelectedRange = null;
    }
  }
  if (touchSelectionAnchor) selectLinearRange(touchSelectionAnchor);
  touchSelectionDragging = Boolean(touchSelectionAnchor);
  mobileSelectionOpen.value = true;
  try { navigator.vibrate?.(12); } catch { /* Haptics are optional. */ }
}

function isMobileSelectionToolbarEvent(event: Event) {
  const target = event.target;
  return target instanceof Node && Boolean(viewRoot.value?.querySelector(".nvx-terminal-view__touch-actions")?.contains(target));
}

/**
 * Whether a scroll step must reach the application as input, exactly like a desktop
 * wheel: mouse-reporting apps get wheel reports and the alternate screen (less, vim,
 * htop without mouse mode) gets cursor keys. Only the normal buffer scrolls locally.
 */
function touchScrollSendsInput() {
  return isMouseReportingEnabled() || terminal?.buffer.active.type === "alternate";
}

/**
 * Scrolls by whole lines (positive = toward newer output). For input-bound modes each
 * line becomes one synthetic line-mode wheel event on the screen element, so xterm's
 * own wheel handlers encode it (mouse protocol or DECCKM-aware arrows) as they do for
 * a desktop wheel; the view never builds those sequences itself. Returns false when
 * the normal buffer is already at the scrollback edge.
 */
function scrollTerminalByLines(lines: number, clientX: number, clientY: number) {
  if (!terminal || !lines) return false;
  if (touchScrollSendsInput()) {
    const screen = terminal.element?.querySelector<HTMLElement>(".xterm-screen");
    if (!screen) return false;
    const steps = Math.min(Math.abs(lines), terminal.rows);
    for (let step = 0; step < steps; step += 1) {
      screen.dispatchEvent(new WheelEvent("wheel", {
        deltaY: Math.sign(lines),
        deltaMode: WheelEvent.DOM_DELTA_LINE,
        clientX,
        clientY,
        bubbles: true,
        cancelable: true,
      }));
    }
    return true;
  }
  const before = terminal.buffer.active.viewportY;
  terminal.scrollLines(lines);
  return terminal.buffer.active.viewportY !== before;
}

function scrollTerminalByPixels(pixels: number, clientX: number, clientY: number) {
  const geometry = terminalScreenGeometry();
  if (!geometry) return false;
  const lines = touchScrollLines.add(pixels, geometry.cellHeight);
  return lines ? scrollTerminalByLines(lines, clientX, clientY) : true;
}

function stopTouchFling() {
  if (!touchFlingFrame) return false;
  cancelAnimationFrame(touchFlingFrame);
  touchFlingFrame = 0;
  touchFlingVelocity = 0;
  return true;
}

/** Inertia after a released drag; scrollback only, so a flick never floods an app with input. */
function startTouchFling(velocity: number) {
  stopTouchFling();
  if (!shouldFling(velocity) || reducedMotionQuery?.matches || touchScrollSendsInput()) return;
  touchFlingVelocity = velocity;
  touchFlingLastAt = performance.now();
  touchFlingFrame = requestAnimationFrame(stepTouchFling);
}

function stepTouchFling(now: number) {
  touchFlingFrame = 0;
  // A buffer switch or mouse mode during the fling ends it instead of turning it into input.
  if (!terminal || touchScrollSendsInput()) return;
  const step = flingStep(touchFlingVelocity, now - touchFlingLastAt);
  touchFlingLastAt = now;
  touchFlingVelocity = step.velocity;
  const geometry = terminalScreenGeometry();
  if (!geometry) return;
  const lines = touchScrollLines.add(step.distance, geometry.cellHeight);
  if (lines && !scrollTerminalByLines(lines, 0, 0)) return;
  if (!step.done) touchFlingFrame = requestAnimationFrame(stepTouchFling);
}

/**
 * Fingers on this terminal surface. A finger on the selection toolbar or outside this
 * view (the extra-keys row, another Pane) never takes part in a terminal gesture.
 */
function surfaceTouches(event: TouchEvent) {
  const root = viewRoot.value;
  const toolbar = root?.querySelector(".nvx-terminal-view__touch-actions") ?? null;
  return Array.from(event.touches).filter((touch) => {
    const target: unknown = touch.target;
    if (!(target instanceof Node)) return true;
    return Boolean(root?.contains(target)) && !toolbar?.contains(target);
  });
}

/** The size xterm currently shows: a pinch preview, else the applied option, else the preference. */
function shownTerminalFontSize() {
  if (pinchPreviewSize !== null) return pinchPreviewSize;
  const applied = terminal?.options.fontSize;
  return typeof applied === "number" ? applied : ui.terminalFontSize;
}

/**
 * Starts, or re-anchors when fingers change, a two-finger pinch. A pinch is never a tap,
 * long-press, selection drag, scroll or fling, and it never raises or dismisses the soft
 * keyboard: the speculative `text` mode of the first finger is undone exactly as for a
 * drag, and the touchend that follows is cancelled so no compatibility mouse event
 * focuses, selects or double-taps.
 */
function beginPinch(fingers: readonly Touch[]) {
  const [first, second] = fingers;
  if (!first || !second) return;
  cancelLongPress();
  stopTouchFling();
  touchStoppedFling = false;
  touchOrigin = null;
  touchSelectionDragging = false;
  touchDragAxis = "pending";
  touchScrollLines.reset();
  if (touchInputModeBefore === "text" || touchInputModeBefore === "none") setMobileInputMode(touchInputModeBefore);
  pinchFingers = [first.identifier, second.identifier];
  pinchTracker.begin(touchSpan(first, second), shownTerminalFontSize());
  pinchIndicatorSize.value = pinchTracker.size;
}

function handlePinchMove(event: TouchEvent) {
  if (event.cancelable) event.preventDefault();
  const tracked = pinchFingers;
  if (touchGesturePhase !== "pinch" || !tracked) return;
  const touches = Array.from(event.touches);
  const first = touches.find((touch) => touch.identifier === tracked[0]);
  const second = touches.find((touch) => touch.identifier === tracked[1]);
  if (!first || !second) return;
  const size = pinchTracker.update(touchSpan(first, second));
  if (size !== null) previewTerminalFontSize(size);
}

/** Live preview: xterm re-measures and refits at most once per frame, only when the integer size changed. */
function previewTerminalFontSize(size: number) {
  pinchPreviewSize = size;
  pinchIndicatorSize.value = size;
  if (!pinchFrame) pinchFrame = requestAnimationFrame(applyPinchPreview);
}

function applyPinchPreview() {
  pinchFrame = 0;
  if (!terminal || pinchPreviewSize === null || terminal.options.fontSize === pinchPreviewSize) return;
  terminal.options.fontSize = pinchPreviewSize;
  // New cell metrics change cols/rows; `fit` reports them through the normal resize event.
  fit();
}

/** Fingers lifted or cancelled during a pinch; the size is persisted once all are up. */
function handlePinchRelease(event: TouchEvent) {
  if (event.cancelable) event.preventDefault();
  const fingers = surfaceTouches(event);
  touchGesturePhase = nextTouchGesturePhase(touchGesturePhase, fingers.length);
  if (touchGesturePhase === "pinch") beginPinch(fingers);
  else if (touchGesturePhase === "idle") finishPinch();
}

function finishPinch() {
  pinchFingers = null;
  pinchIndicatorSize.value = null;
  if (pinchFrame) cancelAnimationFrame(pinchFrame);
  pinchFrame = 0;
  const size = pinchPreviewSize;
  pinchPreviewSize = null;
  if (size === null) return;
  if (terminal && terminal.options.fontSize !== size) {
    terminal.options.fontSize = size;
    fit();
  }
  void persistPinchFontSize(size);
}

/**
 * The pinch result becomes the one terminal font size preference (the same value the
 * Settings field edits), so every terminal view follows it. A failed save restores the
 * stored size in this view and says so, unless a newer pinch already owns the size.
 */
async function persistPinchFontSize(size: number) {
  const attempt = ++pinchCommitSeq;
  if (size === ui.terminalFontSize) return;
  // A rejected save (Core conflict or persistence failure) is handled like a refused one.
  const saved = await ui.setTerminalFontSize(size).catch(() => false);
  if (saved || attempt !== pinchCommitSeq || pinchFingers !== null) return;
  if (terminal && terminal.options.fontSize !== ui.terminalFontSize) {
    terminal.options.fontSize = ui.terminalFontSize;
    scheduleFit();
  }
  tips.show({ scope: `terminal-font-size:${props.paneId ?? "view"}`, tone: "error", title: t("terminalInteraction.fontSizeSaveFailed") });
}

function handleTouchStart(event: TouchEvent) {
  if (isMobileSelectionToolbarEvent(event)) return;
  // Every finger on the screen is new: a gesture whose end this view never saw (hidden,
  // detached) must not leave the next one inert.
  if (event.touches.length === event.changedTouches.length && touchGesturePhase !== "idle") {
    if (pinchFingers !== null) finishPinch();
    touchGesturePhase = "idle";
  }
  const fingers = surfaceTouches(event);
  const phase = nextTouchGesturePhase(touchGesturePhase, fingers.length);
  // Fingers that land together start a pinch directly; the mode it restores is the one found now.
  if (touchGesturePhase === "idle") touchInputModeBefore = terminal?.textarea?.getAttribute("inputmode") ?? null;
  if (phase === "pinch") {
    // A second finger turns any single-finger gesture, including a running scroll, into a pinch.
    touchGesturePhase = "pinch";
    beginPinch(fingers);
    return;
  }
  // The leftover finger of a pinch stays inert until every finger is up.
  if (touchGesturePhase === "settling") return;
  touchGesturePhase = phase;
  // A finger landing on a running fling only stops it; it is neither a tap nor a keyboard request.
  touchStoppedFling = stopTouchFling();
  touchInputModeBefore = terminal?.textarea?.getAttribute("inputmode") ?? null;
  // A finger on the terminal surface is the one deliberate way to the soft keyboard:
  // xterm focuses the textarea on the synthesized mousedown that follows this tap.
  if (!touchStoppedFling) setMobileInputMode("text");
  cancelLongPress();
  touchSelectionDragging = false;
  touchDragAxis = "pending";
  touchScrollLines.reset();
  const touch = fingers.length === 1 ? fingers[0] : null;
  if (!touch) {
    touchOrigin = null;
    return;
  }
  touchOrigin = { identifier: touch.identifier, x: touch.clientX, y: touch.clientY };
  longPressTimer = setTimeout(() => handleLongPress(touch.clientX, touch.clientY), 450);
}

function trackedTouch(event: TouchEvent) {
  if (!touchOrigin) return null;
  return Array.from(event.changedTouches).find((touch) => touch.identifier === touchOrigin?.identifier) ?? null;
}

function handleTouchMove(event: TouchEvent) {
  if (touchGesturePhase === "pinch" || touchGesturePhase === "settling") {
    handlePinchMove(event);
    return;
  }
  const touch = trackedTouch(event);
  if (!touch || !touchOrigin) return;
  if (touchSelectionDragging && touchSelectionAnchor) {
    // After a long-press the same finger extends the selection instead of scrolling the terminal.
    event.preventDefault();
    event.stopPropagation();
    const cell = linearCellAt(touch.clientX, touch.clientY);
    if (cell) selectLinearRange(extendRange(touchSelectionAnchor, cell.offset));
    return;
  }
  if (touchDragAxis === "pending") {
    touchDragAxis = classifyTouchDrag(touch.clientX - touchOrigin.x, touch.clientY - touchOrigin.y);
    if (touchDragAxis === "pending") return;
    // Movement past the slop is a drag: no long-press, and the speculative `text` mode
    // from touchstart is undone so a later programmatic focus cannot raise the keyboard.
    // An open keyboard keeps its mode, so scrolling never dismisses it.
    cancelLongPress();
    if (touchInputModeBefore === "text" || touchInputModeBefore === "none") setMobileInputMode(touchInputModeBefore);
    touchScrollTracker.begin(touch.clientY, event.timeStamp);
  }
  if (touchDragAxis !== "vertical") return;
  // The terminal owns vertical drags (`touch-action: none`); nothing native would scroll it.
  if (event.cancelable) event.preventDefault();
  scrollTerminalByPixels(touchScrollTracker.move(touch.clientY, event.timeStamp), touch.clientX, touch.clientY);
}

function handleTouchEnd(event: TouchEvent) {
  if (touchGesturePhase === "pinch" || touchGesturePhase === "settling") {
    handlePinchRelease(event);
    return;
  }
  if (!surfaceTouches(event).length) touchGesturePhase = "idle";
  const touch = trackedTouch(event);
  if (!touch) return;
  const wasPending = longPressTimer !== null;
  cancelLongPress();
  touchOrigin = null;
  if (touchSelectionDragging) {
    touchSelectionDragging = false;
    // Keep the long-press from turning into a tap that clears the new selection.
    if (event.cancelable) event.preventDefault();
    return;
  }
  if (touchDragAxis !== "pending") {
    const axis = touchDragAxis;
    touchDragAxis = "pending";
    // A drag is never a tap: no compatibility mousedown, so focus, selection and keyboard stay as they were.
    if (event.cancelable) event.preventDefault();
    if (axis === "vertical") startTouchFling(touchScrollTracker.releaseVelocity(event.timeStamp));
    return;
  }
  if (touchStoppedFling) {
    touchStoppedFling = false;
    if (event.cancelable) event.preventDefault();
    return;
  }
  // A plain tap dismisses the selection actions and focuses the terminal as usual.
  if (wasPending && mobileSelectionOpen.value) closeMobileSelection();
  // The tap focuses the helper textarea through xterm; Android opens the soft keyboard only
  // when that textarea is editable at this moment. A read-only view (input ownership still
  // pending for this Pane) remembers the tap so the lease arrival can open the keyboard.
  if (!wasPending) return;
  if (props.readOnly) {
    mobileKeyboardTapAt = Date.now();
    return;
  }
  mobileKeyboardTapAt = 0;
  // An already focused terminal gets no focus change from the tap, so the keyboard would
  // stay closed (dismissed with Back, or focus restored without the keyboard): re-raise it.
  if (document.activeElement === terminal?.textarea && !mobileImeVisible.value) raiseMobileKeyboard();
}

/**
 * Opens the soft keyboard on the already focused textarea. The WebView shows the IME on
 * a focus change of an editable element, not on a tap over the terminal canvas, so the
 * textarea is re-focused once with `inputmode="text"`.
 */
function raiseMobileKeyboard() {
  const textarea = terminal?.textarea;
  if (!textarea || textarea.readOnly || document.activeElement !== textarea) return;
  // The blur arms `inputmode="none"` through the focusout handler; `text` must be set after it.
  textarea.blur();
  setMobileInputMode("text");
  textarea.focus({ preventScroll: true });
  scheduleMobileFocusScrollReset();
}

/**
 * Opens the soft keyboard for a tap that arrived while the view was still read-only.
 * The tap itself focused the readonly textarea, and the WebView decides at focus time
 * not to show the IME; removing `readonly` later does not revisit that decision. A
 * focus change on the now editable textarea does, so the terminal re-focuses itself once
 * within a short window after the tap. Programmatic activation without a tap (Tab
 * switches, returning to the page) never opens the keyboard this way.
 */
const MOBILE_KEYBOARD_TAP_WINDOW_MS = 2500;
function openMobileKeyboardAfterPendingTap() {
  const tappedAt = mobileKeyboardTapAt;
  mobileKeyboardTapAt = 0;
  if (!tappedAt || Date.now() - tappedAt > MOBILE_KEYBOARD_TAP_WINDOW_MS) return;
  raiseMobileKeyboard();
}

/** Focus leaving the textarea (a dialog field, a menu item) means the next focus must not bring the keyboard. */
function handleMobileFocusOut(event: FocusEvent) {
  if (event.target === terminal?.textarea) setMobileInputMode("none");
}

function handleTouchCancel(event: TouchEvent) {
  if (touchGesturePhase === "pinch" || touchGesturePhase === "settling") {
    handlePinchRelease(event);
    return;
  }
  if (!surfaceTouches(event).length) touchGesturePhase = "idle";
  cancelLongPress();
  touchOrigin = null;
  touchSelectionDragging = false;
  touchDragAxis = "pending";
  touchStoppedFling = false;
}

function copyFromMobileSelection() {
  const text = terminal?.getSelection() ?? "";
  closeMobileSelection();
  copySelectionText(text);
}

function pasteFromMobileSelection() {
  closeMobileSelection();
  void pasteFromClipboard();
}

function selectAllFromMobileSelection() {
  terminal?.selectAll();
  touchSelectionAnchor = null;
  touchSelectedRange = null;
  mobileSelectionOpen.value = true;
}

function applyKeyboardOptions(event?: KeyboardEvent) {
  if (!terminal) return;
  const keyboard = preferences.resolvedKeyboard(props.hostId);
  terminal.options.macOptionIsMeta = isMacPlatform() && (event
    ? optionKeyTracker.shouldTreatOptionAsMeta(event, keyboard)
    // When the physical key side is unknown, use the public xterm switch only when both settings permit the safe fallback.
    : keyboard.optionAsMetaLeft && keyboard.optionAsMetaRight);
}

function applyInteractionOptions() {
  if (!terminal) return;
  const interaction = preferences.preferences.interaction;
  terminal.options.scrollback = interaction.scrollback;
  terminal.options.scrollSensitivity = interaction.scrollSensitivity;
  terminal.options.smoothScrollDuration = reducedMotionQuery?.matches ? 0 : interaction.smoothScrollDuration;
  terminal.options.wordSeparator = wordSeparatorForDoubleClickSelection(interaction.doubleClickSelection);
  // OSC 8 follows the same controlled handler; remove it when links close instead of hiding only the custom provider.
  terminal.options.linkHandler = interaction.linksEnabled ? terminalLinkHandler() : null;
  applyKeyboardOptions();
  if (!interaction.linksEnabled) {
    hoveredLinkUrl = null;
    contextLinkUrl.value = null;
  }
}

function effectiveCursorBlink() {
  // Reduced motion collapses xterm's CSS blink animation to 0.01ms; keep the cursor visible and still.
  return ui.terminalCursorBlink && !reducedMotionQuery?.matches;
}

function terminalTheme() {
  const palette = ui.resolvedTerminalPalette;
  return {
    background: palette.background,
    foreground: palette.foreground,
    cursor: palette.cursor,
    selectionBackground: palette.selection,
    black: palette.black,
    red: palette.red,
    green: palette.green,
    yellow: palette.yellow,
    blue: palette.blue,
    magenta: palette.magenta,
    cyan: palette.cyan,
    white: palette.white,
    brightBlack: palette.brightBlack,
    brightRed: palette.brightRed,
    brightGreen: palette.brightGreen,
    brightYellow: palette.brightYellow,
    brightBlue: palette.brightBlue,
    brightMagenta: palette.brightMagenta,
    brightCyan: palette.brightCyan,
    brightWhite: palette.brightWhite,
  };
}

function fitAt(afterOutputSeq: string) {
  if (!terminal || !fitAddon || !host.value || host.value.clientWidth === 0) return;
  if (replayingOutput) return;
  fitAddon.fit();
  recordOutputGeometry(outputGeometry, afterOutputSeq, terminal.rows, terminal.cols);
  if (terminal.rows !== lastRows || terminal.cols !== lastCols) {
    lastRows = terminal.rows;
    lastCols = terminal.cols;
    emit("resize", terminal.rows, terminal.cols);
  }
  if (refreshOnNextFit) {
    refreshOnNextFit = false;
    terminal.refresh(0, terminal.rows - 1);
  }
  scheduleGhost();
}

function fit() {
  const afterOutputSeq = lastOutputSeq;
  if (pendingRenderOperations) {
    pendingRenderOperations += 1;
    lastWriteParsed = lastWriteParsed.then(() => fitAt(afterOutputSeq))
      .finally(() => { pendingRenderOperations -= 1; });
  } else {
    fitAt(afterOutputSeq);
  }
}

function clearGhost() {
  ghostDecoration?.dispose();
  ghostMarker?.dispose();
  ghostDecoration = null;
  ghostMarker = null;
  ghostKey = "";
}

function visibleGhost() {
  const candidate = props.ghostSuggestion;
  if (!terminal || props.readOnly || opaqueInteractive || !simplePrompt || !candidate?.draft || !candidate.suffix
    || draftTracker.value() !== candidate.draft
    || terminal.buffer.active.type !== "normal"
    || !/^[\x20-\x7e]+$/.test(candidate.suffix)) return null;
  const buffer = terminal.buffer.active;
  const cursorX = buffer.cursorX;
  const line = activeLogicalLine();
  const available = terminal.cols - cursorX;
  if (!line || line.start !== simplePrompt.line || available <= 0) return null;
  if (line.beforeCursor !== simplePrompt.prefix + candidate.draft || line.afterCursor.trim().length) return null;
  const preview = candidate.suffix.length <= available
    ? candidate.suffix
    : `${candidate.suffix.slice(0, available - 1)}…`;
  return { candidate, cursorX, line: buffer.baseY + buffer.cursorY, preview };
}

function renderGhost() {
  ghostFrame = 0;
  const position = visibleGhost();
  if (!position || !terminal) { clearGhost(); return; }
  const key = `${position.line}:${position.cursorX}:${position.candidate.draft}:${position.preview}:${ui.resolvedTerminalPalette.foreground}`;
  if (key === ghostKey && ghostDecoration && !ghostDecoration.isDisposed) return;
  clearGhost();
  const marker = terminal.registerMarker(0);
  const decoration = terminal.registerDecoration({ marker, x: position.cursorX, width: position.preview.length });
  if (!decoration) { marker.dispose(); return; }
  ghostMarker = marker;
  ghostDecoration = decoration;
  ghostKey = key;
  const paint = (element: HTMLElement) => {
    element.textContent = position.preview;
    element.style.color = ui.resolvedTerminalPalette.foreground;
    element.style.opacity = "0.55";
    element.style.whiteSpace = "pre";
    element.style.pointerEvents = "none";
    element.setAttribute("aria-hidden", "true");
    element.setAttribute("data-plugin-protected", "");
  };
  decoration.onRender(paint);
  if (decoration.element) paint(decoration.element);
}

function scheduleGhost() {
  cancelAnimationFrame(ghostFrame);
  ghostFrame = requestAnimationFrame(renderGhost);
}

function scheduleFit() {
  cancelAnimationFrame(animationFrame);
  animationFrame = requestAnimationFrame(fit);
}

function handleWorkspaceTabVisibility(event: Event) {
  cancelAnimationFrame(visibleRefreshFrame);
  if ((event as CustomEvent<boolean>).detail !== true) return;
  // The child WebView can receive output while hidden at its initial 1x1 size.
  // Repaint after the native surface has had a frame to become visible;
  // refreshing in the same frame as show() can leave WebKit's surface blank.
  refreshOnNextFit = true;
  scheduleFit();
  visibleRefreshFrame = requestAnimationFrame(() => {
    visibleRefreshFrame = requestAnimationFrame(() => {
      visibleRefreshFrame = 0;
      terminal?.refresh(0, terminal.rows - 1);
    });
  });
}

function writeBytes(bytes: readonly number[], outputSeq?: string) {
  const text = bytes.includes(0x1b) ? String.fromCharCode(...bytes.slice(0, 4096)) : "";
  const csi = `${String.fromCharCode(0x1b)}[`;
  if (["H", "2J"].some((code) => text.includes(csi + code))) {
    // A Shell clear redraws the screen too; it does not make later prompts opaque.
    blockedLine = null;
    invalidateDraft();
  }
  if (!terminal) return;
  const current = terminal;
  const replayGeometry = replayingOutput && outputSeq
    ? geometryForOutput(outputGeometry, outputSeq) : null;
  if (outputSeq) lastOutputSeq = outputSeq;
  pendingRenderOperations += 1;
  lastWriteParsed = lastWriteParsed.then(() => new Promise<void>((resolve) => {
    if (replayGeometry && (current.rows !== replayGeometry.rows || current.cols !== replayGeometry.cols)) {
      current.resize(replayGeometry.cols, replayGeometry.rows);
    }
    current.write(new Uint8Array(bytes), resolve);
  })).finally(() => { pendingRenderOperations -= 1; });
}

function writeGap() {
  invalidateDraft();
  if (terminal) {
    const current = terminal;
    pendingRenderOperations += 1;
    lastWriteParsed = lastWriteParsed.then(() => new Promise<void>((resolve) => current.writeln(`\r\n[${props.gapLabel}]\r\n`, resolve)))
      .finally(() => { pendingRenderOperations -= 1; });
  }
}

function whenOutputParsed() { return lastWriteParsed; }

async function finishReplay() {
  if (!replayingOutput) return;
  await lastWriteParsed;
  replayingOutput = false;
  const latestMarkerSeq = outputGeometry.at(-1)?.afterOutputSeq;
  if (latestMarkerSeq && BigInt(latestMarkerSeq) > BigInt(lastOutputSeq)) lastOutputSeq = latestMarkerSeq;
  fit();
}

function outputGeometrySnapshot() { return outputGeometry.map((marker) => ({ ...marker })); }

function dimensions() {
  return {
    rows: terminal?.rows ?? 24,
    cols: terminal?.cols ?? 80,
  };
}

function focus() {
  // xterm focuses its textarea with preventScroll; phones also undo any keyboard-driven ancestor scroll.
  terminal?.focus();
  if (mobileLayout) scheduleMobileFocusScrollReset();
  clearBellAttention();
}

function clear() {
  terminal?.clear();
  invalidateDraft();
}

function findNext(term: string, incremental = false) {
  if (!searchAddon || !term) {
    searchAddon?.clearDecorations();
    return false;
  }
  return searchAddon.findNext(term, { incremental });
}

function findPrevious(term: string) {
  if (!searchAddon || !term) {
    searchAddon?.clearDecorations();
    return false;
  }
  return searchAddon.findPrevious(term);
}

function clearSearch() {
  searchAddon?.clearDecorations();
}

function selection() {
  return terminal?.getSelection() ?? "";
}

function handlePaste(event: ClipboardEvent) {
  event.preventDefault();
  event.stopImmediatePropagation();
  if (canRequestReconnect() && event.clipboardData?.getData("text/plain")) {
    emit("reconnectRequest");
    return;
  }
  if (props.readOnly) return;
  void pasteGuard.value?.requestPaste(event.clipboardData?.getData("text/plain") ?? "");
}

function pasteFromClipboard() {
  if (canRequestReconnect()) {
    // A paste shortcut is an explicit recovery gesture; do not read or retain its clipboard payload.
    emit("reconnectRequest");
    return;
  }
  if (props.readOnly) return;
  return pasteGuard.value?.pasteFromClipboard();
}

function bracketedPaste() {
  return terminal?.modes.bracketedPasteMode ?? false;
}

function isMouseReportingEnabled() {
  return (terminal?.modes.mouseTrackingMode ?? "none") !== "none";
}

function copyFailure() {
  tips.show({ scope: `terminal-copy:${props.paneId ?? "view"}`, tone: "error", title: t("terminalInteraction.copyFailed") });
}

function copyLinkFailure() {
  tips.show({ scope: `terminal-link-copy:${props.paneId ?? "view"}`, tone: "error", title: t("terminalInteraction.copyLinkFailed") });
}

function copyText(text: string, onFailure: () => void) {
  if (!text) return;
  try { void navigator.clipboard.writeText(text).catch(onFailure); }
  catch { onFailure(); }
}

function copySelectionText(text: string) { copyText(text, copyFailure); }

function resetSelectionMouseGesture() {
  selectionMouseGestureActive = false;
  selectionAtMouseDown = null;
  window.removeEventListener("mouseup", handleSelectionMouseUp);
}

function handleSelectionMouseDown(event: MouseEvent) {
  if (event.button !== 0 || event.shiftKey || isMouseReportingEnabled()) {
    resetSelectionMouseGesture();
    return;
  }
  resetSelectionMouseGesture();
  selectionMouseGestureActive = true;
  selectionAtMouseDown = terminal?.getSelection() ?? "";
  // xterm listens for mousedown, mousemove, and mouseup during mouse gestures; mouseup can land outside the view.
  window.addEventListener("mouseup", handleSelectionMouseUp);
}

function handleSelectionMouseUp(event: MouseEvent) {
  if (!selectionMouseGestureActive || event.button !== 0) return;
  const before = selectionAtMouseDown;
  resetSelectionMouseGesture();
  if (event.shiftKey || !preferences.preferences.interaction.copyOnSelect || isMouseReportingEnabled()) return;
  // Snapshot at the end of this gesture so asynchronous clipboard work never reads a later selection.
  const text = terminal?.getSelection() ?? "";
  if (text !== before) copySelectionText(text);
}

function handleSelectionPointerCancel() { resetSelectionMouseGesture(); }

function handleBellPointerDown() {
  clearBellAttention();
  enableBellAudioFromUserGesture();
}

function handleWindowBlur() {
  resetOptionKeyState();
  resetSelectionMouseGesture();
}

function closeContextMenu(restoreTerminalFocus = false) {
  closeContextMenuPopover();
  contextLinkUrl.value = null;
  if (restoreTerminalFocus) focus();
}

function dismissContextMenuOnTerminalPointerDown(event: PointerEvent) {
  if (!contextMenuOpen.value) return;
  const target = event.target;
  if (target instanceof Node && viewRoot.value?.querySelector(".nvx-terminal-view__context-menu")?.contains(target)) return;
  closeContextMenu();
}

function handleTerminalPointerDown(event: PointerEvent) {
  handleBellPointerDown();
  dismissContextMenuOnTerminalPointerDown(event);
}

function showContextMenuAt(root: HTMLElement, left: number, top: number) {
  const bounds = root.getBoundingClientRect();
  const menuHeight = contextLinkUrl.value ? 136 : 104;
  contextPosition.value = {
    left: Math.max(4, Math.min(left - bounds.left, bounds.width - 184)),
    top: Math.max(4, Math.min(top - bounds.top, bounds.height - menuHeight)),
  };
  if (!contextMenuOpen.value) toggleContextMenu();
}

function showContextMenu(event: MouseEvent) {
  showContextMenuAt(event.currentTarget as HTMLElement, event.clientX, event.clientY);
}

function pasteFromContextMenu() {
  if (props.readOnly) return;
  closeContextMenu();
  // PasteGuard captures this Pane's exact input ticket before clipboard reads and revalidates it before sending.
  void pasteGuard.value?.pasteFromClipboard();
}

function copyFromContextMenu() {
  const text = terminal?.getSelection() ?? "";
  closeContextMenu(true);
  copySelectionText(text);
}

function copyLinkFromContextMenu() {
  const link = contextLinkUrl.value;
  closeContextMenu(true);
  if (link) copyText(link, copyLinkFailure);
}

function selectAllFromContextMenu() {
  terminal?.selectAll();
  closeContextMenu(true);
}

function handleContextMenu(event: MouseEvent) {
  if (mobileLayout) {
    // Phones select by long-press touch tracking; the WebView's own long-press menu never opens.
    event.preventDefault();
    event.stopImmediatePropagation();
    return;
  }
  // TUI mouse reporting and Shift remain with xterm and the remote side; do not intercept and replay another input path here.
  if (event.shiftKey || isMouseReportingEnabled()) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  if (preferences.preferences.interaction.rightClickBehavior === "paste") {
    contextLinkUrl.value = null;
    pasteFromContextMenu();
  } else {
    // The URL was validated on hover; the context menu retains only an immutable snapshot of this click.
    contextLinkUrl.value = hoveredLinkUrl;
    showContextMenu(event);
  }
}

function handleContextMenuShortcut(event: KeyboardEvent) {
  const isContextMenuKey = event.key === "ContextMenu" || (event.shiftKey && event.key === "F10");
  if (!isContextMenuKey || isMouseReportingEnabled()) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  if (mobileLayout) {
    // A hardware keyboard's menu key opens the same in-view selection actions as a long-press.
    mobileSelectionOpen.value = true;
    return;
  }
  const root = event.currentTarget as HTMLElement;
  contextLinkUrl.value = null;
  const bounds = root.getBoundingClientRect();
  showContextMenuAt(root, bounds.left + bounds.width / 2, bounds.top + bounds.height / 2);
}

function handleContextMenuKeyDown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    closeContextMenu(true);
    return;
  }
  handleMenuKeyDown(event);
}

function setBellAttention(active: boolean) {
  if (bellAttentionActive === active) return;
  bellAttentionActive = active;
  emit("bellAttention", active);
}

function clearBellAttention() { setBellAttention(false); }

function terminalHasForegroundFocus() {
  return document.visibilityState === "visible"
    && document.hasFocus()
    && Boolean(viewRoot.value?.contains(document.activeElement));
}

function setViewRoot(element: Element | ComponentPublicInstance | null) {
  rootRef(element);
  viewRoot.value = element instanceof HTMLElement ? element : null;
}

function flashBell() {
  const now = Date.now();
  if (now - lastBellVisualAt < 700) return;
  lastBellVisualAt = now;
  bellFlash.value = true;
  if (bellFlashTimer !== null) clearTimeout(bellFlashTimer);
  bellFlashTimer = window.setTimeout(() => {
    bellFlash.value = false;
    bellFlashTimer = null;
  }, 180);
}

function reportBellAudio(key: "bellSoundGesture" | "bellSoundUnavailable") {
  if (Date.now() - lastBellFeedbackAt < 5_000) return;
  lastBellFeedbackAt = Date.now();
  tips.show({ scope: `terminal-bell:${props.paneId ?? "view"}`, tone: "warning", title: t(`terminalInteraction.${key}`) });
}

function enableBellAudioFromUserGesture() {
  if (preferences.preferences.interaction.bellMode !== "sound") return;
  void bellAudio.enableFromUserGesture().then((availability) => {
    bellAudioAvailability = availability;
    if (availability === "unavailable") reportBellAudio("bellSoundUnavailable");
    else if (availability === "gesture-required") reportBellAudio("bellSoundGesture");
  });
}

function handleBell() {
  const mode = preferences.preferences.interaction.bellMode;
  if (mode === "off") return;
  if (mode === "visual") flashBell();
  if (mode === "sound" && !bellAudio.play()) {
    reportBellAudio(bellAudioAvailability === "unavailable" ? "bellSoundUnavailable" : "bellSoundGesture");
  }
  if (!terminalHasForegroundFocus()) setBellAttention(true);
}

function handleTerminalKeyDownCapture(event: KeyboardEvent) {
  clearBellAttention();
  enableBellAudioFromUserGesture();
  handleContextMenuShortcut(event);
  const position = visibleGhost();
  if (event.type === "keydown" && event.key === "Tab" && !event.repeat && !event.isComposing
    && !event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey
    && props.paneId && terminalHasForegroundFocus() && !document.querySelector('[role="dialog"][aria-modal="true"]')
    && position && ghostDecoration?.element && !ghostDecoration.isDisposed
    && ghostKey === `${position.line}:${position.cursorX}:${position.candidate.draft}:${position.preview}:${ui.resolvedTerminalPalette.foreground}`
    && captureTerminalInput(props.paneId)) {
    event.preventDefault();
    event.stopImmediatePropagation();
    emit("acceptSuggestion");
  }
}

function handleTerminalFocusIn() {
  clearBellAttention();
  if (mobileLayout) scheduleMobileFocusScrollReset();
}
function resetOptionKeyState() { optionKeyTracker.reset(); }

function activateTerminalLink(event: MouseEvent, candidate: string) {
  // While xterm reports mouse input, every click belongs to the remote TUI; links must never intercept or double-send it.
  if (isMouseReportingEnabled()
    || !preferences.preferences.interaction.linksEnabled
    || !terminalLinkModifierPressed(event, detectDesktopPlatform())) return;
  const snapshot = safeTerminalHttpUrl(candidate);
  if (!snapshot) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  void openSafeTerminalHttpUrl(snapshot).then((opened) => {
    if (!opened) tips.show({ scope: `terminal-link-open:${props.paneId ?? "view"}`, tone: "error", title: t("terminalInteraction.openLinkFailed") });
  }).catch(() => {
    tips.show({ scope: `terminal-link-open:${props.paneId ?? "view"}`, tone: "error", title: t("terminalInteraction.openLinkFailed") });
  });
}

function hoverTerminalLink(candidate: string) {
  hoveredLinkUrl = preferences.preferences.interaction.linksEnabled ? safeTerminalHttpUrl(candidate) : null;
}

function leaveTerminalLink(candidate: string) {
  if (hoveredLinkUrl === safeTerminalHttpUrl(candidate)) hoveredLinkUrl = null;
}

function terminalLinkHandler() {
  return {
    allowNonHttpProtocols: false,
    activate: (event: MouseEvent, text: string) => activateTerminalLink(event, text),
    hover: (_event: MouseEvent, text: string) => hoverTerminalLink(text),
    leave: (_event: MouseEvent, text: string) => leaveTerminalLink(text),
  };
}

function bufferColumnForTextOffset(line: IBufferLine, textOffset: number) {
  let column = 0;
  let offset = 0;
  while (column < line.length && offset < textOffset) {
    const cell = line.getCell(column);
    if (!cell) break;
    const width = cell.getWidth();
    if (width <= 0) {
      column += 1;
      continue;
    }
    const chars = cell.getChars();
    if (offset + chars.length > textOffset) break;
    offset += chars.length;
    column += width;
  }
  return column;
}

function terminalHttpLinkProvider(): ILinkProvider {
  return {
    provideLinks(bufferLineNumber, callback) {
      if (!terminal || !preferences.preferences.interaction.linksEnabled || isMouseReportingEnabled()) {
        callback(undefined);
        return;
      }
      const line = terminal.buffer.active.getLine(bufferLineNumber - 1);
      if (!line) {
        callback(undefined);
        return;
      }
      const links = findTerminalHttpLinks(line.translateToString(true)).flatMap((link): ILink[] => {
        const startColumn = bufferColumnForTextOffset(line, link.start);
        const endColumn = bufferColumnForTextOffset(line, link.end);
        if (endColumn <= startColumn) return [];
        const snapshot = link.url;
        return [{
          text: snapshot,
          range: {
            start: { x: startColumn + 1, y: bufferLineNumber },
            end: { x: endColumn, y: bufferLineNumber },
          },
          decorations: { pointerCursor: true, underline: true },
          activate: (event) => activateTerminalLink(event, snapshot),
          hover: () => hoverTerminalLink(snapshot),
          leave: () => leaveTerminalLink(snapshot),
        }];
      });
      callback(links.length ? links : undefined);
    },
  };
}

function handleTerminalCustomKeyEvent(event: KeyboardEvent) {
  optionKeyTracker.observe(event);
  if (event.defaultPrevented) return false;
  // On phones the IME text path owns composition and keyCode-229 events; xterm's
  // textarea diffing would duplicate or drop that text. Real key codes still reach xterm.
  if (mobileLayout && (event.keyCode === 229 || event.key === "Process" || mobileIme?.isComposing())) return false;
  applyKeyboardOptions(event);
  if (canRequestReconnect()
    && event.type === "keydown"
    && !event.repeat && !event.metaKey && !event.ctrlKey && !event.altKey
    && (Array.from(event.key).length === 1 || ["Enter", "Dead", "Process"].includes(event.key))) {
    // Keep xterm read-only: consume the gesture, never replay its bytes into the new Shell.
    event.preventDefault();
    event.stopImmediatePropagation();
    emit("reconnectRequest");
    return false;
  }
  if (props.readOnly) return false;
  const keyboard = preferences.resolvedKeyboard(props.hostId);
  if (keyboard.backspaceMode === "bs" && isPlainBackspace(event)) {
    event.preventDefault();
    event.stopImmediatePropagation();
    invalidateDraft();
    emit("input", "\b");
    return false;
  }
  return true;
}

function canRequestReconnect() {
  return props.readOnly && props.reconnectOnInput
    && terminal?.textarea === document.activeElement
    && terminalHasForegroundFocus()
    && !document.querySelector('[role="dialog"][aria-modal="true"]');
}

function invalidateDraft() { simplePrompt = null; shellTabCandidate = null; clearGhost(); emit("draftChange", draftTracker.invalidate()); }
function blockDraftLine() {
  blockedLine = activeLogicalLine()?.start ?? null;
  invalidateDraft();
}
function appendDraftText(text: string) { clearGhost(); emit("draftChange", draftTracker.input(text)); }
function currentDraft() { return draftTracker.value(); }

const emptyPromptPatterns = [
  /^\[[A-Za-z0-9_.-]+@[A-Za-z0-9_.:-]+ [^\r\n]{1,80}\][#$] ?$/,
  /^[A-Za-z0-9_.-]+@[A-Za-z0-9_.-]+:[^\r\n#$]{1,80}[#$] ?$/,
  /^[A-Za-z0-9_.-]+@[A-Za-z0-9_.-]+ [^\r\n]{1,80}% ?$/,
  /^PS (?:[A-Za-z]:\\|\\\\|\/)[^\r\n]{0,160}>\s*$/,
];

function activeLogicalLine() {
  if (!terminal || terminal.buffer.active.type !== "normal") return null;
  const buffer = terminal.buffer.active;
  const current = buffer.baseY + buffer.cursorY;
  const line = buffer.getLine(current);
  if (!line) return null;
  let start = current;
  let beforeCursor = line.translateToString(false, 0, buffer.cursorX);
  while (start > 0 && buffer.getLine(start)?.isWrapped) {
    const previous = buffer.getLine(--start);
    if (!previous) return null;
    beforeCursor = previous.translateToString(false, 0, terminal.cols) + beforeCursor;
    // Only bounded prompt and command text belongs to a simple Shell line.
    if (beforeCursor.length > 4096) return null;
  }
  return { start, beforeCursor, afterCursor: line.translateToString(false, buffer.cursorX, terminal.cols) };
}

function visibleShellCommand() {
  const line = activeLogicalLine();
  if (!line || line.afterCursor.trim()) return null;
  const maximumPromptLength = Math.min(line.beforeCursor.length - 1, 180);
  for (let length = 1; length <= maximumPromptLength; length++) {
    let prefix = line.beforeCursor.slice(0, length);
    if (!emptyPromptPatterns.some((pattern) => pattern.test(prefix))) continue;
    // A Shell prompt's optional trailing space belongs to the prompt, not
    // to the command; do not absorb later '#' characters from the command.
    if (line.beforeCursor[length] === " " && emptyPromptPatterns.some((pattern) => pattern.test(prefix + " "))) prefix += " ";
    const command = line.beforeCursor.slice(prefix.length);
    if (/^[\x20-\x7e]{1,4096}$/.test(command)) return { line: line.start, prefix, command };
  }
  return null;
}

function isEmptyShellPrompt() {
  if (!terminal || opaqueInteractive || terminal.buffer.active.type !== "normal") return false;
  return hasEmptyShellPrompt();
}

function hasEmptyShellPrompt() {
  const line = activeLogicalLine();
  if (!line || blockedLine === line.start || line.afterCursor.trim()) return false;
  // Recognize only strong Shell prompt forms. Ambiguous TUI input is never sampled.
  return emptyPromptPatterns.some((pattern) => pattern.test(line.beforeCursor));
}

function observeBufferState() {
  if (!terminal) return;
  if (terminal.buffer.active.type === "alternate") {
    if (!alternateActive) {
      alternateActive = true;
      alternateEntryLine = terminal.buffer.normal.baseY + terminal.buffer.normal.cursorY;
    }
    awaitingShellAfterAlternate = false;
    opaqueInteractive = true;
    invalidateDraft();
    return;
  }
  if (alternateActive) {
    alternateActive = false;
    awaitingShellAfterAlternate = true;
    blockedLine = null;
    invalidateDraft();
  }
  // Leaving the alternate buffer restores old screen contents. A later,
  // newly drawn Shell prompt is needed before input can be observed again.
  const line = activeLogicalLine();
  if (awaitingShellAfterAlternate && line && alternateEntryLine !== null
    && line.start > alternateEntryLine && hasEmptyShellPrompt()) {
    opaqueInteractive = false;
    awaitingShellAfterAlternate = false;
    alternateEntryLine = null;
    blockedLine = null;
  }
}

function simpleCommandAtCursor() {
  const draft = draftTracker.value();
  if (!terminal || !simplePrompt || !draft || terminal.buffer.active.type !== "normal") return null;
  const line = activeLogicalLine();
  if (!line || line.start !== simplePrompt.line) return null;
  const visible = line.beforeCursor;
  if (!visible.startsWith(simplePrompt.prefix)
    || !draft.startsWith(visible.slice(simplePrompt.prefix.length))) return null;
  if (line.afterCursor.trim()) return null;
  return draft;
}

function shellTabCommandAtCursor() {
  if (!simplePrompt || !shellTabCandidate) return null;
  const line = activeLogicalLine();
  if (!line || line.afterCursor.trim() || !line.beforeCursor.startsWith(simplePrompt.prefix)) return null;
  const command = line.beforeCursor.slice(simplePrompt.prefix.length);
  // Tab completion comes from the Shell. Only capture its echoed result when
  // it still contains the typed prefix and all printable input after Tab.
  if (!command.startsWith(shellTabCandidate.prefix)
    || !command.endsWith(shellTabCandidate.typedAfterTab)
    || !/^[\x20-\x7e]{1,4096}$/.test(command)) return null;
  return command;
}

function observeSimpleInput(value: string) {
  if (!terminal || props.readOnly) return null;
  if (value === "\r" || value === "\n") {
    const command = shellTabCandidate ? shellTabCommandAtCursor() : simpleCommandAtCursor();
    const launchCandidate = command ?? draftTracker.value();
    invalidateDraft();
    if (launchCandidate && /(?:^|[;&|\s/])(?:codex|claude)(?:[-\w]*)(?:[;&|\s]|$)/i.test(launchCandidate)) opaqueInteractive = true;
    return command;
  }
  if (value === "\t" && !opaqueInteractive) {
    const visible = visibleShellCommand();
    if (visible && blockedLine !== visible.line && (!simplePrompt || visible.prefix === simplePrompt.prefix)) {
      const draft = shellTabCandidate ? null : draftTracker.value();
      const typedPrefix = draft && draft.startsWith(visible.command) ? draft : visible.command;
      simplePrompt = { line: visible.line, prefix: visible.prefix };
      shellTabCandidate = { prefix: typedPrefix, typedAfterTab: "" };
      clearGhost();
      emit("draftChange", null);
      return null;
    }
  }
  if (opaqueInteractive || !/^[\x20-\x7e]$/.test(value)) {
    blockedLine = activeLogicalLine()?.start ?? terminal.buffer.active.baseY + terminal.buffer.active.cursorY;
    invalidateDraft();
    return null;
  }
  if (shellTabCandidate) {
    shellTabCandidate.typedAfterTab += value;
    return null;
  }
  if (!simplePrompt) {
    if (!isEmptyShellPrompt()) { invalidateDraft(); return null; }
    const line = activeLogicalLine()!;
    simplePrompt = { line: line.start, prefix: line.beforeCursor };
    draftTracker.prompt();
  } else if (activeLogicalLine()?.start !== simplePrompt.line) {
    invalidateDraft();
    return null;
  }
  appendDraftText(value);
  return null;
}

defineExpose({
  writeBytes,
  writeGap,
  whenOutputParsed,
  finishReplay,
  outputGeometrySnapshot,
  dimensions,
  focus,
  fit,
  clear,
  findNext,
  findPrevious,
  clearSearch,
  selection,
  pasteFromClipboard,
  appendDraftText,
  invalidateDraft,
  currentDraft,
  applicationCursorKeys,
});

onMounted(async () => {
  window.addEventListener("blur", handleWindowBlur);
  if (typeof window.matchMedia === "function") {
    reducedMotionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
  }
  window.addEventListener("norishell:workspace-tab-visibility", handleWorkspaceTabVisibility);
  terminal = new Terminal({
    ...(props.initialOutputGeometry?.[0]
      ? { rows: props.initialOutputGeometry[0].rows, cols: props.initialOutputGeometry[0].cols }
      : props.initialDimensions ? { rows: props.initialDimensions.rows, cols: props.initialDimensions.cols } : {}),
    allowProposedApi: true,
    convertEol: false,
    customGlyphs: true,
    cursorBlink: effectiveCursorBlink(),
    cursorStyle: ui.terminalCursorStyle,
    disableStdin: props.readOnly,
    fontFamily: terminalFontCssFamily(ui.terminalFontFamily),
    fontSize: ui.terminalFontSize,
    fontWeight: ui.terminalFontWeight,
    fontWeightBold: ui.terminalBoldFontWeight,
    letterSpacing: ui.terminalLetterSpacing,
    lineHeight: ui.terminalLineHeight,
    scrollback: preferences.preferences.interaction.scrollback,
    scrollSensitivity: preferences.preferences.interaction.scrollSensitivity,
    smoothScrollDuration: 0,
    wordSeparator: wordSeparatorForDoubleClickSelection(preferences.preferences.interaction.doubleClickSelection),
    macOptionIsMeta: false,
    linkHandler: terminalLinkHandler(),
    theme: terminalTheme(),
  });
  replayingOutput = Boolean(props.initialOutputGeometry?.length);
  if (!outputGeometry.length) recordOutputGeometry(outputGeometry, "0", terminal.rows, terminal.cols);
  fitAddon = new FitAddon();
  searchAddon = new SearchAddon();
  terminal.loadAddon(fitAddon);
  terminal.loadAddon(searchAddon);
  if (reducedMotionQuery) {
    const updateForMotionPreference = () => {
      applyInteractionOptions();
      if (terminal) terminal.options.cursorBlink = effectiveCursorBlink();
    };
    reducedMotionQuery.addEventListener("change", updateForMotionPreference);
    removeReducedMotionListener = () => reducedMotionQuery?.removeEventListener("change", updateForMotionPreference);
  }
  applyInteractionOptions();
  terminal.onData((rawValue) => {
    if (!props.readOnly) {
      // Phone helper-row Ctrl/Alt apply to the next on-screen keyboard character; desktop data is untouched.
      const value = mobileLayout ? applyStickyModifiers(rawValue) : rawValue;
      // Typing ends a touch selection the same way it clears xterm's own selection.
      if (mobileSelectionOpen.value) closeMobileSelection(false);
      const observedCommand = observeSimpleInput(value);
      if (observedCommand) emit("input", value, observedCommand);
      else emit("input", value);
    }
  });
  selectionChangeDisposable = terminal.onSelectionChange(() => {
    if (mobileLayout) mobileSelectionText.value = terminal?.getSelection() ?? "";
    emit("selectionChange", terminal?.hasSelection() ?? false);
  });
  bellDisposable = terminal.onBell(handleBell);
  // Application shortcuts are handled centrally during window capture; no second non-configurable mapping may remain.
  terminal.attachCustomKeyEventHandler(handleTerminalCustomKeyEvent);
  if (host.value) terminal.open(host.value);
  if (mobileLayout) {
    applyMobileTextareaAttributes(terminal.textarea);
    if (viewRoot.value) {
      mobileIme = createMobileImeInput({
        eventRoot: viewRoot.value,
        textarea: () => terminal?.textarea,
        deliver: deliverMobileText,
        paste: deliverMobilePaste,
        backspace: backspaceSequence,
        preview: showCompositionPreview,
      });
      viewRoot.value.addEventListener("touchstart", handleTouchStart, { capture: true, passive: true });
      viewRoot.value.addEventListener("touchmove", handleTouchMove, { capture: true, passive: false });
      viewRoot.value.addEventListener("touchend", handleTouchEnd, { capture: true, passive: false });
      viewRoot.value.addEventListener("touchcancel", handleTouchCancel, { capture: true });
      viewRoot.value.addEventListener("focusout", handleMobileFocusOut, true);
    }
    document.addEventListener("scroll", handleMobileAncestorScroll, true);
    window.visualViewport?.addEventListener("resize", handleMobileViewportChange);
  }
  linkProviderDisposable = terminal.registerLinkProvider(terminalHttpLinkProvider());
  highlighter = createTerminalHighlighter(terminal, (reason) => { highlightFailure.value = reason; });
  highlighter.update(preferences.resolvedHighlights(props.hostId));
  ghostWriteListener = terminal.onWriteParsed(() => {
    observeBufferState();
    scheduleGhost();
  });
  ghostResizeListener = terminal.onResize(scheduleGhost);
  observer = new ResizeObserver(scheduleFit);
  if (host.value) observer.observe(host.value);
  await nextTick();
  fit();
  scheduleGhost();
});

watch(() => [props.ghostSuggestion, props.readOnly], scheduleGhost, { deep: true, flush: "post" });

// The keyboard closed (Back key, system) while the terminal kept focus: the next
// programmatic focus must not bring it back; only a tap does.
watch(mobileImeVisible, (visible) => {
  if (mobileLayout && !visible) setMobileInputMode("none");
});

watch(
  () => preferences.resolvedHighlights(props.hostId),
  (configuration) => highlighter?.update(configuration),
  { deep: true },
);

// Update public xterm options only; never rebuild the Terminal, session, attachment, or replay.
watch(
  () => preferences.preferences.interaction,
  () => applyInteractionOptions(),
  { deep: true },
);

watch(
  () => preferences.resolvedKeyboard(props.hostId),
  () => applyKeyboardOptions(),
  { deep: true },
);

watch(
  () => props.readOnly,
  (readOnly) => {
    // Text composed before ownership was lost must not be committed into a read-only view later.
    if (readOnly) {
      discardingImeText = true;
      try { mobileIme?.flush(); } finally { discardingImeText = false; }
    }
    if (terminal) terminal.options.disableStdin = readOnly;
    // After input ownership is interrupted, another view or approved input may change the Shell editing area; wait for the next prompt before inferring again.
    if (readOnly) invalidateDraft();
    else if (mobileLayout) openMobileKeyboardAfterPendingTap();
  },
);

watch(
  () => [
    ui.terminalFontFamily,
    ui.terminalFontSize,
    ui.terminalFontWeight,
    ui.terminalBoldFontWeight,
    ui.terminalLineHeight,
    ui.terminalLetterSpacing,
  ] as const,
  async ([
    fontFamily,
    fontSize,
    fontWeight,
    boldFontWeight,
    lineHeight,
    letterSpacing,
  ]) => {
    if (!terminal) return;
    terminal.options.fontFamily = terminalFontCssFamily(fontFamily);
    terminal.options.fontSize = fontSize;
    terminal.options.fontWeight = fontWeight;
    terminal.options.fontWeightBold = boldFontWeight;
    terminal.options.lineHeight = lineHeight;
    terminal.options.letterSpacing = letterSpacing;
    await nextTick();
    scheduleFit();
  },
);

watch(
  () => [ui.terminalCursorStyle, ui.terminalCursorBlink] as const,
  ([cursorStyle]) => {
    if (!terminal) return;
    terminal.options.cursorStyle = cursorStyle;
    terminal.options.cursorBlink = effectiveCursorBlink();
  },
);

watch(
  () => [ui.theme, ui.terminalThemeMode, ui.customTerminalPalette, ui.resolvedTerminalPalette],
  async () => {
    await nextTick();
    if (terminal) terminal.options.theme = terminalTheme();
    scheduleGhost();
  },
  { deep: true },
);

onBeforeUnmount(() => {
  cancelLongPress();
  stopTouchFling();
  // An unfinished pinch is dropped with the view; only a completed gesture is persisted.
  cancelAnimationFrame(pinchFrame);
  pinchFrame = 0;
  pinchFingers = null;
  pinchPreviewSize = null;
  cancelAnimationFrame(mobileScrollResetFrame);
  document.removeEventListener("scroll", handleMobileAncestorScroll, true);
  window.visualViewport?.removeEventListener("resize", handleMobileViewportChange);
  mobileIme?.dispose();
  mobileIme = null;
  selectionChangeDisposable?.dispose();
  selectionChangeDisposable = null;
  if (viewRoot.value) {
    viewRoot.value.removeEventListener("touchstart", handleTouchStart, { capture: true });
    viewRoot.value.removeEventListener("touchmove", handleTouchMove, { capture: true });
    viewRoot.value.removeEventListener("touchend", handleTouchEnd, { capture: true });
    viewRoot.value.removeEventListener("touchcancel", handleTouchCancel, { capture: true });
    viewRoot.value.removeEventListener("focusout", handleMobileFocusOut, true);
  }
  cancelAnimationFrame(visibleRefreshFrame);
  cancelAnimationFrame(ghostFrame);
  clearGhost();
  ghostWriteListener?.dispose();
  ghostResizeListener?.dispose();
  window.removeEventListener("blur", handleWindowBlur);
  window.removeEventListener("norishell:workspace-tab-visibility", handleWorkspaceTabVisibility);
  resetSelectionMouseGesture();
  cancelAnimationFrame(animationFrame);
  if (bellFlashTimer !== null) clearTimeout(bellFlashTimer);
  bellFlashTimer = null;
  bellDisposable?.dispose();
  bellDisposable = null;
  linkProviderDisposable?.dispose();
  linkProviderDisposable = null;
  bellAudio.dispose();
  setBellAttention(false);
  observer?.disconnect();
  removeReducedMotionListener?.();
  highlighter?.dispose();
  highlighter = null;
  terminal?.dispose();
  observer = null;
  terminal = null;
  fitAddon = null;
  searchAddon = null;
  reducedMotionQuery = null;
  removeReducedMotionListener = null;
});
</script>

<template>
  <div
    :ref="setViewRoot"
    class="nvx-terminal-view"
    :class="{ 'nvx-terminal-view--bell-flash': bellFlash }"
    data-plugin-protected
    role="application"
    :aria-label="terminalLabel"
    @paste.capture="handlePaste"
    @pointerdown.capture="handleTerminalPointerDown"
    @mousedown.capture="handleSelectionMouseDown"
    @pointercancel="handleSelectionPointerCancel"
    @contextmenu.capture="handleContextMenu"
    @keydown.capture="handleTerminalKeyDownCapture"
    @focusin="handleTerminalFocusIn"
  >
    <div
      ref="host"
      class="nvx-terminal-view__host"
    />
    <span
      v-if="highlightFailure"
      class="nvx-terminal-view__highlight-error"
      role="status"
    >{{ t(`terminalEnhancements.highlightSuspended.${highlightFailure}`) }}</span>
    <span
      v-if="pinchIndicatorSize !== null"
      class="nvx-terminal-view__font-size-indicator"
      role="status"
      aria-live="polite"
    >{{ t('terminalInteraction.fontSizeIndicator', { size: pinchIndicatorSize }) }}</span>
    <div
      v-if="contextMenuOpen"
      class="nvx-terminal-view__context-menu"
      role="menu"
      :aria-label="t('terminalInteraction.contextMenu')"
      :style="{ left: `${contextPosition.left}px`, top: `${contextPosition.top}px` }"
      @keydown="handleContextMenuKeyDown"
    >
      <button
        class="nvx-terminal-view__context-menu-item"
        type="button"
        role="menuitem"
        :disabled="!selection()"
        @click="copyFromContextMenu"
      >
        {{ t('terminalInteraction.contextCopy') }}
      </button>
      <button
        v-if="contextLinkUrl"
        class="nvx-terminal-view__context-menu-item"
        type="button"
        role="menuitem"
        @click="copyLinkFromContextMenu"
      >
        {{ t('terminalInteraction.contextCopyLink') }}
      </button>
      <button
        class="nvx-terminal-view__context-menu-item"
        type="button"
        role="menuitem"
        :disabled="readOnly || !paneId"
        @click="pasteFromContextMenu"
      >
        {{ t('terminalInteraction.contextPaste') }}
      </button>
      <button
        class="nvx-terminal-view__context-menu-item"
        type="button"
        role="menuitem"
        @click="selectAllFromContextMenu"
      >
        {{ t('terminalInteraction.contextSelectAll') }}
      </button>
    </div>
    <div
      v-if="mobileSelectionOpen"
      class="nvx-terminal-view__touch-actions"
      role="toolbar"
      :aria-label="t('terminalInteraction.contextMenu')"
    >
      <!-- Pointer-down is cancelled so the terminal keeps keyboard focus and its input lease. -->
      <button
        class="nvx-terminal-view__touch-action"
        type="button"
        :disabled="!mobileSelectionText"
        @pointerdown.prevent
        @click="copyFromMobileSelection"
      >
        {{ t('terminalInteraction.contextCopy') }}
      </button>
      <button
        class="nvx-terminal-view__touch-action"
        type="button"
        :disabled="(readOnly && !reconnectOnInput) || !paneId"
        @pointerdown.prevent
        @click="pasteFromMobileSelection"
      >
        {{ t('terminalInteraction.contextPaste') }}
      </button>
      <button
        class="nvx-terminal-view__touch-action"
        type="button"
        @pointerdown.prevent
        @click="selectAllFromMobileSelection"
      >
        {{ t('terminalInteraction.contextSelectAll') }}
      </button>
      <button
        class="nvx-terminal-view__touch-action nvx-terminal-view__touch-action--close"
        type="button"
        :aria-label="t('mobile.closeSheet')"
        @pointerdown.prevent
        @click="closeMobileSelection()"
      >
        <NvxIcon
          :icon="X"
          :size="20"
        />
      </button>
    </div>
    <NvxTerminalPasteGuard
      v-if="paneId"
      ref="pasteGuard"
      :pane-id="paneId"
      :bracketed="bracketedPaste"
      @focus="focus"
      @pasted="blockDraftLine"
    />
  </div>
</template>

<style scoped>
.nvx-terminal-view {
  position: relative;
  box-sizing: border-box;
  width: 100%;
  height: 100%;
  min-height: 0;
  padding: 8px 5px;
  contain: layout paint;
  overflow: hidden;
  background: var(--nvx-color-terminal-bg, #101217);
}

.nvx-terminal-view--bell-flash::after {
  position: absolute;
  z-index: 2;
  inset: 2px;
  border: 2px solid var(--nvx-color-warning, #d7a531);
  border-radius: var(--nvx-radius-sm);
  content: "";
  pointer-events: none;
  animation: nvx-terminal-bell-flash 180ms ease-out;
}

@keyframes nvx-terminal-bell-flash {
  from { opacity: 0.95; }
  to { opacity: 0; }
}

@media (prefers-reduced-motion: reduce) {
  .nvx-terminal-view--bell-flash::after { animation: none; opacity: 0.55; }
}

.nvx-terminal-view__context-menu {
  position: absolute;
  z-index: var(--nvx-z-popover);
  display: grid;
  width: min(180px, calc(100% - 8px));
  padding: 2px;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-overlay);
}

.nvx-terminal-view__context-menu-item {
  min-height: 28px;
  padding: 4px var(--nvx-space-2);
  border: 0;
  border-radius: var(--nvx-radius-sm);
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  font-size: var(--nvx-font-size-sm);
  text-align: left;
}

.nvx-terminal-view__context-menu-item:hover:not(:disabled),
.nvx-terminal-view__context-menu-item:focus-visible {
  background: var(--nvx-color-bg-subtle);
  outline: none;
}

.nvx-terminal-view__context-menu-item:disabled {
  color: var(--nvx-color-text-tertiary);
}

/* Phone selection actions stay inside the terminal surface, above the first rows. */
.nvx-terminal-view__touch-actions {
  position: absolute;
  z-index: var(--nvx-z-popover);
  top: var(--nvx-space-2);
  right: var(--nvx-space-2);
  left: var(--nvx-space-2);
  display: flex;
  gap: var(--nvx-space-1);
  justify-content: flex-end;
  pointer-events: none;
}

.nvx-terminal-view__touch-action {
  display: inline-flex;
  min-width: 48px;
  min-height: 44px;
  align-items: center;
  justify-content: center;
  padding: 0 var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-overlay);
  color: var(--nvx-color-text-primary);
  font: inherit;
  font-size: var(--nvx-font-size-sm);
  pointer-events: auto;
}

.nvx-terminal-view__touch-action--close {
  padding: 0;
}

.nvx-terminal-view__touch-action:disabled {
  color: var(--nvx-color-text-tertiary);
}

.nvx-terminal-view__touch-action:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 1px;
}

.nvx-terminal-view__highlight-error {
  position: absolute;
  top: 4px;
  right: 8px;
  max-width: calc(100% - 16px);
  padding: 2px 6px;
  border: 1px solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-secondary);
  font-size: 11px;
}

.nvx-terminal-view__font-size-indicator {
  position: absolute;
  top: 8px;
  left: 50%;
  transform: translateX(-50%);
  padding: 4px 10px;
  border: 1px solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  font-size: 13px;
  font-variant-numeric: tabular-nums;
  pointer-events: none;
}

.nvx-terminal-view :deep(.xterm) {
  box-sizing: border-box;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--nvx-color-terminal-bg, #101217);
}

/* FitAddon reads the direct parent's height; leave spacing outside the measurement container. */
.nvx-terminal-view__host {
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
}

.nvx-terminal-view :deep(.xterm-viewport),
.nvx-terminal-view :deep(.xterm-screen) {
  background-color: var(--nvx-color-terminal-bg, #101217);
}

.nvx-terminal-view :deep(.xterm-helper-textarea) {
  background: transparent;
  color: var(--nvx-color-terminal-fg);
}

.nvx-terminal-view :deep(.composition-view) {
  background: var(--nvx-color-terminal-bg, #101217);
  color: var(--nvx-color-terminal-fg);
}

/* A long press opens the application's terminal menu, never the WebView's own callout.
   xterm 6 has no natively scrollable element, so the view handles every drag itself
   (scrollback, wheel input, selection); the WebView must not claim pans or zoom. */
html[data-nvx-host="mobile"] .nvx-terminal-view,
html[data-nvx-host="mobile"] .nvx-terminal-view :deep(.xterm) {
  -webkit-touch-callout: none;
  touch-action: none;
}
</style>
