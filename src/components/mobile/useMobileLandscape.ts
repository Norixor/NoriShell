import { computed, onBeforeUnmount, onMounted, ref, type ComputedRef, type Ref } from "vue";
import { useRoute } from "vue-router";

import { isMobileLayout } from "../../platform-host";
import { useWorkspaceTabsStore } from "../../stores/workspaceTabs";

/**
 * Phone orientation as a reactive fact. Only the phone shell consults it: the
 * desktop frame never changes with the window aspect, so on desktop this stays
 * `false` and registers no media listener.
 */
export function useMobileLandscape(): Ref<boolean> {
  const landscape = ref(false);
  let query: MediaQueryList | null = null;
  const update = () => { landscape.value = query?.matches ?? false; };
  onMounted(() => {
    if (!isMobileLayout() || typeof window.matchMedia !== "function") return;
    query = window.matchMedia("(orientation: landscape)");
    query.addEventListener("change", update);
    update();
  });
  onBeforeUnmount(() => {
    query?.removeEventListener("change", update);
    query = null;
  });
  return landscape;
}

/**
 * "Terminal focus" mode: a landscape phone showing the Terminal page with at
 * least one Terminal Tab. The bottom navigation collapses and the top bar compacts
 * so the terminal gets the rows; portrait and every other page are unchanged.
 * Switching Tabs, pages or orientation only changes layout, never a Session.
 */
export function useMobileTerminalFocusMode(): ComputedRef<boolean> {
  const landscape = useMobileLandscape();
  const route = useRoute();
  const workspaceTabs = useWorkspaceTabsStore();
  return computed(() => landscape.value && route.path === "/terminal" && workspaceTabs.terminalTabs.length > 0);
}

const IME_ATTRIBUTE = "data-nvx-ime";
const IME_EVENT = "norishell:ime-visibility";

/**
 * Whether the soft keyboard is open, as the Android Activity reports it: MainActivity
 * sets `html[data-nvx-ime="visible"]` and dispatches `norishell:ime-visibility`
 * (`detail.visible`) from the window-inset listener. The page cannot tell a keyboard
 * from any other viewport shrink, so this is the only source. Always `false` on desktop
 * and wherever the native signal does not exist (browser preview).
 */
export function useMobileImeVisible(): Ref<boolean> {
  const visible = ref(false);
  const read = () => { visible.value = document.documentElement.getAttribute(IME_ATTRIBUTE) === "visible"; };
  const update = (event: Event) => {
    const detail = (event as CustomEvent<{ visible?: unknown }>).detail;
    if (typeof detail?.visible === "boolean") visible.value = detail.visible;
    else read();
  };
  onMounted(() => {
    if (!isMobileLayout()) return;
    window.addEventListener(IME_EVENT, update);
    read();
  });
  onBeforeUnmount(() => {
    window.removeEventListener(IME_EVENT, update);
  });
  return visible;
}

/** The visual viewport height in CSS px (the Android WebView shrinks with the soft keyboard). */
export function useMobileViewportHeight(): Ref<number> {
  const height = ref(0);
  const update = () => { height.value = Math.round(window.visualViewport?.height ?? window.innerHeight); };
  onMounted(() => {
    if (!isMobileLayout()) return;
    window.visualViewport?.addEventListener("resize", update);
    window.addEventListener("resize", update);
    update();
  });
  onBeforeUnmount(() => {
    window.visualViewport?.removeEventListener("resize", update);
    window.removeEventListener("resize", update);
  });
  return height;
}

/**
 * Below this height a landscape phone has the soft keyboard open (a landscape
 * viewport without it is at least ~360px; with it about 175px). The compact bar
 * and the navigation handle would then leave the terminal a single row.
 */
export const MOBILE_IMMERSIVE_MAX_HEIGHT = 300;

/**
 * "Immersive" terminal: focus mode with the soft keyboard open. The top bar and
 * the navigation handle hide entirely; only the terminal and its helper keys stay.
 * Closing the keyboard (or rotating) restores the focus-mode layout. The keyboard is
 * known from the native IME fact; the viewport-height rule remains as the fallback
 * where that signal does not exist.
 */
export function useMobileTerminalImmersive(focusMode: ComputedRef<boolean>): ComputedRef<boolean> {
  const viewportHeight = useMobileViewportHeight();
  const imeVisible = useMobileImeVisible();
  return computed(() => focusMode.value
    && (imeVisible.value || (viewportHeight.value > 0 && viewportHeight.value < MOBILE_IMMERSIVE_MAX_HEIGHT)));
}
