<script setup lang="ts">
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Terminal, type IBufferLine, type IDisposable, type ILink, type ILinkProvider } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { nextTick, onBeforeUnmount, onMounted, ref, watch, type ComponentPublicInstance } from "vue";
import { useI18n } from "vue-i18n";

import { useUiStore } from "../../stores/ui";
import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";
import { useTipsStore } from "../../stores/tips";
import { detectDesktopPlatform } from "../../platform";
import { terminalFontCssFamily } from "../../terminal-theme";
import { createTerminalHighlighter } from "../../terminal/xtermHighlighting";
import { TerminalDraftTracker } from "../../terminal/draft";
import { wordSeparatorForDoubleClickSelection } from "../../terminal/interaction-preferences";
import { MacOptionKeyTracker, isPlainBackspace } from "../../terminal/keyboard-compatibility";
import { TerminalBellAudio, type TerminalBellAudioAvailability } from "../../terminal/terminal-bell";
import { findTerminalHttpLinks, openSafeTerminalHttpUrl, safeTerminalHttpUrl, terminalLinkModifierPressed } from "../../terminal/terminal-links";
import { usePopoverMenu } from "../ui/usePopoverMenu";
import NvxTerminalPasteGuard from "./NvxTerminalPasteGuard.vue";

const props = defineProps<{
  readOnly: boolean;
  reconnectOnInput?: boolean;
  terminalLabel: string;
  gapLabel: string;
  paneId?: string;
  hostId?: string | null;
  shellPromptKey?: string | null;
}>();

const emit = defineEmits<{
  input: [value: string];
  reconnectRequest: [];
  resize: [rows: number, cols: number];
  selectionChange: [hasSelection: boolean];
  searchRequest: [];
  draftChange: [draft: string | null];
  bellAttention: [active: boolean];
}>();

const host = ref<HTMLElement | null>(null);
const viewRoot = ref<HTMLElement | null>(null);
const ui = useUiStore();
const { t } = useI18n();
const preferences = useTerminalPreferencesStore();
const tips = useTipsStore();
const pasteGuard = ref<InstanceType<typeof NvxTerminalPasteGuard> | null>(null);
const highlightFailed = ref(false);
const contextPosition = ref({ left: 0, top: 0 });
const bellFlash = ref(false);
const contextLinkUrl = ref<string | null>(null);
const { rootRef, open: contextMenuOpen, closeMenu: closeContextMenuPopover, toggleMenu: toggleContextMenu, handleMenuKeyDown } = usePopoverMenu();
let highlighter: ReturnType<typeof createTerminalHighlighter> | null = null;
const draftTracker = new TerminalDraftTracker();
let terminal: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let searchAddon: SearchAddon | null = null;
let observer: ResizeObserver | null = null;
let animationFrame = 0;
let lastRows = 0;
let lastCols = 0;
let reducedMotionQuery: MediaQueryList | null = null;
let removeReducedMotionListener: (() => void) | null = null;
let selectionMouseGestureActive = false;
let selectionAtMouseDown: string | null = null;
let bellDisposable: IDisposable | null = null;
let linkProviderDisposable: IDisposable | null = null;
let bellFlashTimer: number | null = null;
let lastBellVisualAt = 0;
let lastBellFeedbackAt = 0;
let bellAudioAvailability: TerminalBellAudioAvailability | null = null;
let hoveredLinkUrl: string | null = null;
let bellAttentionActive = false;
const optionKeyTracker = new MacOptionKeyTracker();
const bellAudio = new TerminalBellAudio();

function isMacPlatform() { return detectDesktopPlatform() === "macos"; }

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

function fit() {
  if (!terminal || !fitAddon || !host.value || host.value.clientWidth === 0) return;
  fitAddon.fit();
  if (terminal.rows !== lastRows || terminal.cols !== lastCols) {
    lastRows = terminal.rows;
    lastCols = terminal.cols;
    emit("resize", terminal.rows, terminal.cols);
  }
}

function scheduleFit() {
  cancelAnimationFrame(animationFrame);
  animationFrame = requestAnimationFrame(fit);
}

function writeBytes(bytes: readonly number[]) {
  terminal?.write(new Uint8Array(bytes));
}

function writeGap() {
  invalidateDraft();
  terminal?.writeln(`\r\n[${props.gapLabel}]\r\n`);
}

function dimensions() {
  return {
    rows: terminal?.rows ?? 24,
    cols: terminal?.cols ?? 80,
  };
}

function focus() {
  terminal?.focus();
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
}

function handleTerminalFocusIn() { clearBellAttention(); }
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
    appendDraftText("\b");
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

function invalidateDraft() { emit("draftChange", draftTracker.invalidate()); }
function appendDraftText(text: string) { emit("draftChange", draftTracker.input(text)); }
function currentDraft() { return draftTracker.value(); }

defineExpose({
  writeBytes,
  writeGap,
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
});

onMounted(async () => {
  window.addEventListener("blur", handleWindowBlur);
  terminal = new Terminal({
    allowProposedApi: true,
    convertEol: false,
    customGlyphs: true,
    cursorBlink: ui.terminalCursorBlink,
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
  fitAddon = new FitAddon();
  searchAddon = new SearchAddon();
  terminal.loadAddon(fitAddon);
  terminal.loadAddon(searchAddon);
  if (typeof window.matchMedia === "function") {
    reducedMotionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
    const updateForMotionPreference = () => applyInteractionOptions();
    reducedMotionQuery.addEventListener("change", updateForMotionPreference);
    removeReducedMotionListener = () => reducedMotionQuery?.removeEventListener("change", updateForMotionPreference);
  }
  applyInteractionOptions();
  terminal.onData((value) => {
    if (!props.readOnly) {
      appendDraftText(value);
      emit("input", value);
    }
  });
  terminal.onSelectionChange(() => {
    emit("selectionChange", terminal?.hasSelection() ?? false);
  });
  bellDisposable = terminal.onBell(handleBell);
  // Application shortcuts are handled centrally during window capture; no second non-configurable mapping may remain.
  terminal.attachCustomKeyEventHandler(handleTerminalCustomKeyEvent);
  if (host.value) terminal.open(host.value);
  linkProviderDisposable = terminal.registerLinkProvider(terminalHttpLinkProvider());
  highlighter = createTerminalHighlighter(terminal, (failed) => { highlightFailed.value = failed; });
  highlighter.update(preferences.resolvedHighlights(props.hostId));
  observer = new ResizeObserver(scheduleFit);
  if (host.value) observer.observe(host.value);
  await nextTick();
  fit();
});

watch(
  () => props.shellPromptKey,
  (key) => emit("draftChange", key ? draftTracker.prompt() : draftTracker.invalidate()),
  { immediate: true },
);

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
    if (terminal) terminal.options.disableStdin = readOnly;
    // After input ownership is interrupted, another view or approved input may change the Shell editing area; wait for the next prompt before inferring again.
    if (readOnly) invalidateDraft();
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
    ui.terminalCursorStyle,
    ui.terminalCursorBlink,
  ] as const,
  async ([
    fontFamily,
    fontSize,
    fontWeight,
    boldFontWeight,
    lineHeight,
    letterSpacing,
    cursorStyle,
    cursorBlink,
  ]) => {
    if (!terminal) return;
    terminal.options.fontFamily = terminalFontCssFamily(fontFamily);
    terminal.options.fontSize = fontSize;
    terminal.options.fontWeight = fontWeight;
    terminal.options.fontWeightBold = boldFontWeight;
    terminal.options.lineHeight = lineHeight;
    terminal.options.letterSpacing = letterSpacing;
    terminal.options.cursorStyle = cursorStyle;
    terminal.options.cursorBlink = cursorBlink;
    await nextTick();
    scheduleFit();
  },
);

watch(
  () => [ui.theme, ui.terminalThemeMode, ui.customTerminalPalette, ui.resolvedTerminalPalette],
  async () => {
    await nextTick();
    if (terminal) terminal.options.theme = terminalTheme();
  },
  { deep: true },
);

onBeforeUnmount(() => {
  window.removeEventListener("blur", handleWindowBlur);
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
    @pointerdown.capture="handleBellPointerDown"
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
      v-if="highlightFailed"
      class="nvx-terminal-view__highlight-error"
      role="status"
    >{{ t('terminalEnhancements.highlightSuspended') }}</span>
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
    <NvxTerminalPasteGuard
      v-if="paneId"
      ref="pasteGuard"
      :pane-id="paneId"
      :bracketed="bracketedPaste"
      @focus="focus"
      @pasted="invalidateDraft"
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
  color: var(--nvx-color-text-disabled);
}

.nvx-terminal-view__highlight-error {
  position: absolute;
  top: 4px;
  right: 8px;
  max-width: calc(100% - 16px);
  padding: 2px 6px;
  border: 1px solid var(--nvx-color-border-subtle);
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-secondary);
  font-size: 11px;
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
</style>
