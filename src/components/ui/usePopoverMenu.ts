import type { ComponentPublicInstance, CSSProperties } from "vue";
import { nextTick, onBeforeUnmount, onMounted, ref } from "vue";

interface ComponentTrigger {
  $el?: HTMLElement;
}

/**
  * Centralizes the host Popover Menu lifecycle for opening, focus, keyboard interaction, and outside dismissal.
  * Each calling component still owns its menu actions and visible items.
 */
export function usePopoverMenu(options: { enabled?: boolean } = {}) {
  const root = ref<HTMLElement | null>(null);
  const trigger = ref<HTMLElement | ComponentTrigger | null>(null);
  const viewportPanel = ref<HTMLElement | null>(null);
  const viewportPanelStyle = ref<CSSProperties>({ visibility: "hidden" });
  const open = ref(false);
  let visibilityObserver: ResizeObserver | null = null;

  function menuItems() {
    return Array.from(
      (viewportPanel.value ?? root.value)?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? [],
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
    if (restoreFocus) void nextTick(() => triggerElement()?.focus());
  }

  function toggleMenu() {
    open.value = !open.value;
    if (open.value) void (async () => {
      await nextTick();
      if (viewportPanel.value) {
        await positionViewportPanel();
        await nextTick();
      }
      menuItems()[0]?.focus();
    })();
    else viewportPanelStyle.value = { visibility: "hidden" };
  }

  function handleDocumentPointerDown(event: PointerEvent) {
    if (open.value && !root.value?.contains(event.target as Node) && !viewportPanel.value?.contains(event.target as Node)) closeMenu();
  }

  function dismissForViewportChange(event: Event) {
    if (event.target instanceof Node && viewportPanel.value?.contains(event.target)) return;
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
    if (options.enabled === false) return;
    document.removeEventListener("pointerdown", handleDocumentPointerDown);
    document.removeEventListener("keydown", handleDocumentKeyDown);
    window.removeEventListener("resize", dismissForViewportChange);
    window.removeEventListener("scroll", dismissForViewportChange, true);
    visibilityObserver?.disconnect();
  });

  return { rootRef, triggerRef, viewportPanelRef, viewportPanelStyle, open, closeMenu, toggleMenu, handleMenuKeyDown };
}
