import { computed, type ComputedRef } from "vue";
import { useRoute } from "vue-router";

import { isMobileLayout } from "../../platform-host";
import { useWorkspaceTabsStore } from "../../stores/workspaceTabs";
import { useMobileLandscape, useMobileTerminalImmersive } from "../mobile/useMobileLandscape";

/**
 * Whether the phone shell is in immersive terminal mode (landscape Terminal page
 * with the soft keyboard open — the same rule the shell uses to hide its top bar).
 * Pane toolbars then collapse to a thin state-colored strip so the terminal keeps
 * its rows; closing the keyboard restores them. Always false on desktop, where no
 * listener is registered.
 */
export function useTerminalPaneImmersive(): ComputedRef<boolean> {
  if (!isMobileLayout()) return computed(() => false);
  const landscape = useMobileLandscape();
  // A Pane only renders inside the Terminal page; without a router (isolated mounts) treat it as that page.
  const route = useRoute() as ReturnType<typeof useRoute> | undefined;
  const workspaceTabs = useWorkspaceTabsStore();
  const focusMode = computed(() => landscape.value
    && (route ? route.path === "/terminal" : true)
    && workspaceTabs.terminalTabs.length > 0);
  return useMobileTerminalImmersive(focusMode);
}
