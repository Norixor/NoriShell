import type { ComponentPublicInstance, CSSProperties } from "vue";
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { mountMobileSheetBackdrop, pushMobileOverlay, type MobileOverlayHandle, type MobileSheetBackdropHandle } from "../../mobile-overlays";
import { isMobileLayout } from "../../platform-host";

interface ComponentTrigger {
  $el?: HTMLElement;
}

/**
  * Centralizes the host Popover Menu lifecycle for opening, focus, keyboard interaction, and outside dismissal.
  * Each calling component still owns its menu actions and visible items.
  *
  * On the phone shell the same menu renders as a bottom action sheet: the panel is
  * tagged `data-nvx-mobile-sheet` (styled globally), gets a scrim and joins the
  * overlay stack so the Android back key closes it. While open, the panel is hosted
  * directly under `body`: an ancestor with `contain`, `transform` or `filter` would
  * otherwise become the containing block of the fixed sheet and trap it beneath the
  * body-level scrim. The panel returns to its place before Vue patches the consumer.
  * Desktop positioning is unchanged.
 */
export function usePopoverMenu(options: { enabled?: boolean } = {}) {
  const root = ref<HTMLElement | null>(null);
  const trigger = ref<HTMLElement | ComponentTrigger | null>(null);
  const viewportPanel = ref<HTMLElement | null>(null);
  const viewportPanelStyle = ref<CSSProperties>({ visibility: "hidden" });
  const open = ref(false);
  let visibilityObserver: ResizeObserver | null = null;
  const mobileSheet = isMobileLayout();
  let overlay: MobileOverlayHandle | null = null;
  let removeBackdrop: MobileSheetBackdropHandle | null = null;
  let hostedPanel: HTMLElement | null = null;
  let hostedPlaceholder: Comment | null = null;

  function hostPanelInBody(panel: HTMLElement) {
    if (hostedPanel || panel.parentNode === document.body || !panel.parentNode) return;
    hostedPlaceholder = document.createComment("nvx-mobile-sheet");
    panel.parentNode.insertBefore(hostedPlaceholder, panel);
    document.body.appendChild(panel);
    hostedPanel = panel;
  }

  function restoreHostedPanel() {
    const panel = hostedPanel;
    const placeholder = hostedPlaceholder;
    hostedPanel = null;
    hostedPlaceholder = null;
    if (!placeholder) return;
    // A panel Vue already removed stays removed; only a live one goes back for Vue to unmount in place.
    if (panel?.isConnected && placeholder.parentNode) placeholder.parentNode.insertBefore(panel, placeholder);
    placeholder.remove();
  }

  function presentMobileSheet() {
    const panel = viewportPanel.value ?? root.value?.querySelector<HTMLElement>('[role="menu"]') ?? null;
    if (panel) {
      panel.dataset.nvxMobileSheet = "";
      hostPanelInBody(panel);
    }
    if (viewportPanel.value) viewportPanelStyle.value = {};
    removeBackdrop ??= mountMobileSheetBackdrop(() => closeMenu(true));
    overlay ??= pushMobileOverlay(() => { closeMenu(true); return true; }, () => [removeBackdrop?.element, panel]);
    overlay.refresh();
  }

  function dismissMobileSheet() {
    restoreHostedPanel();
    removeBackdrop?.();
    removeBackdrop = null;
    overlay?.release();
    overlay = null;
  }

  // Consumers that write `open` directly still get the panel back before their re-render.
  if (mobileSheet) watch(open, (value) => { if (!value) dismissMobileSheet(); }, { flush: "pre" });

  function menuItems() {
    return Array.from(
      (viewportPanel.value ?? hostedPanel ?? root.value)?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? [],
    );
  }

  function triggerElement() {
    const value = trigger.value;
    return value instanceof HTMLElement ? value : value?.$el;
  }

  function rootRef(element: Element | ComponentPublicInstance | null) {
    root.value = element instanceof HTMLElement ? element : null;
  }

  function triggerRef(element: Element | ComponentPublicInstance | null) {
    if (element instanceof HTMLElement) trigger.value = element;
    else trigger.value = element && "$el" in element ? element as ComponentTrigger : null;
  }

  function viewportPanelRef(element: Element | ComponentPublicInstance | null) {
    viewportPanel.value = element instanceof HTMLElement ? element : null;
  }

  async function positionViewportPanel() {
    await nextTick();
    const anchor = triggerElement()?.getBoundingClientRect();
    const panel = viewportPanel.value;
    if (!open.value || !anchor || !panel) return;

    const margin = 8;
    const gap = 4;
    const width = panel.getBoundingClientRect().width;
    const height = panel.getBoundingClientRect().height;
    const below = window.innerHeight - anchor.bottom - margin;
    const above = anchor.top - margin;
    const top = below < height && above > below
      ? Math.max(margin, anchor.top - height - gap)
      : Math.min(anchor.bottom + gap, window.innerHeight - height - margin);
    viewportPanelStyle.value = {
      top: `${Math.max(margin, top)}px`,
      left: `${Math.max(margin, Math.min(anchor.right - width, window.innerWidth - width - margin))}px`,
    };
  }

  function closeMenu(restoreFocus = false) {
    open.value = false;
    viewportPanelStyle.value = { visibility: "hidden" };
    if (mobileSheet) dismissMobileSheet();
    if (restoreFocus) void nextTick(() => triggerElement()?.focus());
  }

  function toggleMenu() {
    open.value = !open.value;
    if (open.value) void (async () => {
      await nextTick();
      if (mobileSheet) {
        presentMobileSheet();
        await nextTick();
      } else if (viewportPanel.value) {
        await positionViewportPanel();
        await nextTick();
      }
      menuItems()[0]?.focus();
    })();
    else {
      viewportPanelStyle.value = { visibility: "hidden" };
      if (mobileSheet) dismissMobileSheet();
    }
  }

  function handleDocumentPointerDown(event: PointerEvent) {
    const target = event.target as Node;
    if (open.value && !root.value?.contains(target) && !viewportPanel.value?.contains(target) && !hostedPanel?.contains(target)) closeMenu();
  }

  function dismissForViewportChange(event: Event) {
    // A phone sheet is anchored to the viewport bottom by CSS, so a viewport change never
    // invalidates it. Opening the sheet moves focus off the terminal, which closes the soft
    // keyboard and resizes the window; dismissing here would make the sheet vanish before
    // it is seen and the tap look swallowed.
    if (mobileSheet && event.type === "resize") return;
    if (event.target instanceof Node && (viewportPanel.value?.contains(event.target) || hostedPanel?.contains(event.target))) return;
    if (viewportPanel.value && open.value) closeMenu();
  }

  function handleDocumentKeyDown(event: KeyboardEvent) {
    if (open.value && event.key === "Escape") {
      event.preventDefault();
      closeMenu(true);
    }
  }

  function handleMenuKeyDown(event: KeyboardEvent) {
    const items = menuItems();
    if (!items.length) return;
    const currentIndex = Math.max(0, items.indexOf(document.activeElement as HTMLButtonElement));
    let nextIndex: number | null = null;
    if (event.key === "ArrowDown") nextIndex = (currentIndex + 1) % items.length;
    else if (event.key === "ArrowUp") nextIndex = (currentIndex - 1 + items.length) % items.length;
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = items.length - 1;
    else if (event.key === "Tab") closeMenu();
    if (nextIndex === null) return;
    event.preventDefault();
    items[nextIndex]?.focus();
  }

  onMounted(() => {
    if (options.enabled === false) return;
    document.addEventListener("pointerdown", handleDocumentPointerDown);
    document.addEventListener("keydown", handleDocumentKeyDown);
    window.addEventListener("resize", dismissForViewportChange);
    window.addEventListener("scroll", dismissForViewportChange, true);
    if (typeof ResizeObserver !== "undefined" && root.value) {
      visibilityObserver = new ResizeObserver(() => {
        if (open.value && root.value && getComputedStyle(root.value).display === "none") closeMenu();
      });
      visibilityObserver.observe(root.value);
    }
  });

  onBeforeUnmount(() => {
    dismissMobileSheet();
    if (options.enabled === false) return;
    document.removeEventListener("pointerdown", handleDocumentPointerDown);
    document.removeEventListener("keydown", handleDocumentKeyDown);
    window.removeEventListener("resize", dismissForViewportChange);
    window.removeEventListener("scroll", dismissForViewportChange, true);
    visibilityObserver?.disconnect();
  });

  return { rootRef, triggerRef, viewportPanelRef, viewportPanelStyle, open, closeMenu, toggleMenu, handleMenuKeyDown };
}
