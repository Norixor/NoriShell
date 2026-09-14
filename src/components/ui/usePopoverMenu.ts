import type { ComponentPublicInstance } from "vue";
import { nextTick, onBeforeUnmount, onMounted, ref } from "vue";

interface ComponentTrigger {
  $el?: HTMLElement;
}

/**
  * Centralizes the host Popover Menu lifecycle for opening, focus, keyboard interaction, and outside dismissal.
  * Each calling component still owns its menu actions and visible items.
 */
export function usePopoverMenu() {
  const root = ref<HTMLElement | null>(null);
  const trigger = ref<HTMLElement | ComponentTrigger | null>(null);
  const open = ref(false);
  let visibilityObserver: ResizeObserver | null = null;

  function menuItems() {
    return Array.from(
      root.value?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? [],
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

  function closeMenu(restoreFocus = false) {
    open.value = false;
    if (restoreFocus) void nextTick(() => triggerElement()?.focus());
  }

  function toggleMenu() {
    open.value = !open.value;
    if (open.value) void nextTick(() => menuItems()[0]?.focus());
  }

  function handleDocumentPointerDown(event: PointerEvent) {
    if (open.value && !root.value?.contains(event.target as Node)) closeMenu();
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
    document.addEventListener("pointerdown", handleDocumentPointerDown);
    document.addEventListener("keydown", handleDocumentKeyDown);
    if (typeof ResizeObserver !== "undefined" && root.value) {
      visibilityObserver = new ResizeObserver(() => {
        if (open.value && root.value && getComputedStyle(root.value).display === "none") closeMenu();
      });
      visibilityObserver.observe(root.value);
    }
  });

  onBeforeUnmount(() => {
    document.removeEventListener("pointerdown", handleDocumentPointerDown);
    document.removeEventListener("keydown", handleDocumentKeyDown);
    visibilityObserver?.disconnect();
  });

  return { rootRef, triggerRef, open, closeMenu, toggleMenu, handleMenuKeyDown };
}
