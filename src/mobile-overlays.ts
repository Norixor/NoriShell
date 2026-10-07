/**
 * Overlay stack for the phone shell. Every transient surface (dialog/sheet, menu,
 * select listbox, Tab switcher) registers while open; the Android back key closes
 * the topmost one. The stack itself is platform-neutral and cheap, so components
 * register unconditionally; only `installMobileBackHandler` is Android-specific.
 *
 * The stack also owns stacking order: each entry may name the elements that make
 * up its layer (a sheet and its scrim, or a dialog backdrop), and the stack writes
 * `--nvx-mobile-overlay-z` on them by push order (two z-index steps per entry, the
 * scrim below the sheet). A surface opened from inside another overlay therefore
 * always lands above it, whatever static z-index its stylesheet would give it.
 *
 * Contract consumed by the native side (`MainActivity`): `window.__norishellHandleBack()`
 * returns `true` when the page consumed the key. A blocking dialog (not dismissible)
 * still consumes it without closing, so the WebView never navigates history or
 * backgrounds the task underneath a confirmation that must be answered.
 */
import { readonly, ref } from "vue";

export interface MobileOverlayHandle {
  /** Removes the entry; safe to call more than once. */
  release: () => void;
  /** Re-applies the stacking order, e.g. after the overlay's elements have rendered. */
  refresh: () => void;
}

/** Elements that form one overlay's layer; `null` entries (not rendered yet) are skipped. */
export type MobileOverlayLayers = () => Array<HTMLElement | null | undefined>;

interface OverlayEntry {
  id: number;
  /** Returns true when the key was consumed (closed or intentionally kept open). */
  onBack: () => boolean;
  layers: MobileOverlayLayers | null;
  styled: Set<HTMLElement>;
  /**
   * In-page navigation levels (a list → detail page) join the back stack but are
   * not overlays: they take no z slot and do not count as an open surface for
   * components that lay out around overlays (Tips stay at the bottom).
   */
  navigation: boolean;
}

declare global {
  interface Window {
    __norishellHandleBack?: () => boolean;
  }
}

export const MOBILE_OVERLAY_Z_PROPERTY = "--nvx-mobile-overlay-z";
/** Mirrors `--nvx-z-dialog` in tokens.css; read live so a theme override still wins. */
const DEFAULT_OVERLAY_Z_BASE = 40;
const OVERLAY_Z_STEP = 2;

const stack: OverlayEntry[] = [];
let sequence = 0;
// Reactive mirror of the stack size for components that lay out around open overlays (Tips).
const depth = ref(0);
export const mobileOverlayDepthRef = readonly(depth);

function overlayZBase(): number {
  if (typeof getComputedStyle !== "function" || !document.documentElement) return DEFAULT_OVERLAY_Z_BASE;
  const value = Number.parseInt(getComputedStyle(document.documentElement).getPropertyValue("--nvx-z-dialog"), 10);
  return Number.isFinite(value) ? value : DEFAULT_OVERLAY_Z_BASE;
}

function overlayDepth(): number {
  return stack.reduce((count, entry) => count + (entry.navigation ? 0 : 1), 0);
}

function applyLayers() {
  const base = overlayZBase();
  let level = 0;
  stack.forEach((entry) => {
    if (entry.navigation) return;
    const index = level++;
    if (!entry.layers) return;
    const z = String(base + index * OVERLAY_Z_STEP);
    const current = new Set<HTMLElement>();
    for (const element of entry.layers()) {
      if (!element) continue;
      element.style.setProperty(MOBILE_OVERLAY_Z_PROPERTY, z);
      current.add(element);
    }
    for (const element of entry.styled) {
      if (!current.has(element)) element.style.removeProperty(MOBILE_OVERLAY_Z_PROPERTY);
    }
    entry.styled = current;
  });
}

function pushEntry(onBack: () => boolean, layers: MobileOverlayLayers | null, navigation: boolean): MobileOverlayHandle {
  sequence += 1;
  const entry: OverlayEntry = { id: sequence, onBack, layers, styled: new Set(), navigation };
  stack.push(entry);
  depth.value = overlayDepth();
  applyLayers();
  // Elements rendered by the same tick (Teleported dialogs, v-if panels) are styled once they exist.
  if (layers) queueMicrotask(applyLayers);
  return {
    release: () => {
      const index = stack.findIndex((item) => item.id === entry.id);
      if (index < 0) return;
      stack.splice(index, 1);
      depth.value = overlayDepth();
      for (const element of entry.styled) element.style.removeProperty(MOBILE_OVERLAY_Z_PROPERTY);
      entry.styled.clear();
      applyLayers();
    },
    refresh: applyLayers,
  };
}

export function pushMobileOverlay(onBack: () => boolean, layers?: MobileOverlayLayers): MobileOverlayHandle {
  return pushEntry(onBack, layers ?? null, false);
}

/**
 * Registers an in-page navigation level (e.g. the Settings section detail on the
 * phone). The back key reaches it only once every overlay opened above it has
 * closed; `onBack` returns to the parent level and reports the key as consumed.
 */
export function pushMobileBackTarget(onBack: () => boolean): MobileOverlayHandle {
  return pushEntry(onBack, null, true);
}

/** Number of overlays currently registered, navigation levels excluded (tests and diagnostics). */
export function mobileOverlayDepth(): number {
  return overlayDepth();
}

/** Number of back-stack entries including navigation levels (tests and diagnostics). */
export function mobileBackStackDepth(): number {
  return stack.length;
}

/** Handles one back press: the topmost overlay decides; an empty stack returns false. */
export function handleMobileBack(): boolean {
  const top = stack.at(-1);
  if (!top) return false;
  return top.onBack();
}

/** Exposes the back handler to the native shell; returns the uninstaller. */
export function installMobileBackHandler(target: Window = window): () => void {
  target.__norishellHandleBack = handleMobileBack;
  return () => {
    if (target.__norishellHandleBack === handleMobileBack) delete target.__norishellHandleBack;
  };
}

/** Test hook: clears every registered overlay. */
export function resetMobileOverlaysForTests(): void {
  for (const entry of stack) {
    for (const element of entry.styled) element.style.removeProperty(MOBILE_OVERLAY_Z_PROPERTY);
  }
  stack.splice(0);
  depth.value = 0;
}

/** The remover of a sheet scrim, which also exposes the scrim element for the overlay's layer. */
export type MobileSheetBackdropHandle = (() => void) & { element: HTMLElement };

/**
 * A full-viewport scrim for bottom-sheet menus that are not dialogs. The sheet
 * itself stays owned by its component; the scrim only dismisses on tap.
 */
export function mountMobileSheetBackdrop(onDismiss: () => void): MobileSheetBackdropHandle {
  const backdrop = document.createElement("div");
  backdrop.className = "nvx-mobile-sheet-backdrop";
  backdrop.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    onDismiss();
  });
  document.body.appendChild(backdrop);
  return Object.assign(() => backdrop.remove(), { element: backdrop });
}
