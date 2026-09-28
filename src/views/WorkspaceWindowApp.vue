<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch, type VNodeRef } from "vue";
import { useRoute, useRouter } from "vue-router";

import { NvxAppHeader, NvxWindowFrame, NvxWorkspaceTabBar, NvxWorkspaceTabPlaceholder } from "../components/layout";
import { NvxTips } from "../components/ui";
import { useUiStore } from "../stores/ui";
import { useTipsStore } from "../stores/tips";
import { i18n } from "../locales";
import { startWorkspaceTabWindowUi } from "../workspace-tab-window-ui";
import { setWorkspaceTabViewContentBounds } from "../workspace-tab-view-shell";
import { useRouteMotion } from "../route-motion";
import { startNativeBackgroundSync } from "../native-window-background";

const ui = useUiStore();
const router = useRouter();
const route = useRoute();
const setRouteMotionElement = useRouteMotion(() => route.path);
const tips = useTipsStore();
const workspaceContent = ref<HTMLElement | null>(null);
const setWorkspaceContentElement: VNodeRef = (node, refs) => {
  workspaceContent.value = node instanceof HTMLElement ? node : null;
  if (typeof setRouteMotionElement === "function") setRouteMotionElement(node, refs);
};
let workspaceContentObserver: ResizeObserver | null = null;
function syncWorkspaceContentBounds() {
  const bounds = workspaceContent.value?.getBoundingClientRect();
  if (!bounds) return;
  const zoom = ui.appliedUiZoom / 100;
  setWorkspaceTabViewContentBounds({
    x: bounds.x * zoom,
    y: bounds.y * zoom,
    width: bounds.width * zoom,
    height: bounds.height * zoom,
  });
}
watch(() => ui.appliedUiZoom, () => { void Promise.resolve().then(syncWorkspaceContentBounds); });
let stopWorkspaceTabWindows: (() => void) | null = null;
let stopNativeBackground: (() => void) | null = null;
let disposed = false;

ui.applyPreferences();
void ui.setUiZoom(ui.uiZoom, false);
onMounted(() => {
  stopNativeBackground = startNativeBackgroundSync("window-and-webview");
  workspaceContentObserver = new ResizeObserver(syncWorkspaceContentBounds);
  if (workspaceContent.value) workspaceContentObserver.observe(workspaceContent.value);
  window.addEventListener("resize", syncWorkspaceContentBounds);
  syncWorkspaceContentBounds();
  void startWorkspaceTabWindowUi(router).then((stop) => {
    if (disposed) stop(); else stopWorkspaceTabWindows = stop;
  }).catch(() => {
    tips.show({ scope: "workspace-tab-windows", tone: "error", title: i18n.global.t("workspaceTabs.unavailable") });
  });
});
onBeforeUnmount(() => {
  workspaceContentObserver?.disconnect();
  window.removeEventListener("resize", syncWorkspaceContentBounds);
  setWorkspaceTabViewContentBounds(null);
  disposed = true;
  stopNativeBackground?.();
  stopWorkspaceTabWindows?.();
});
</script>

<template>
  <NvxWindowFrame
    v-slot="{ platform }"
    :zoom="ui.appliedUiZoom / 100"
  >
    <NvxTips />
    <div class="workspace-window-shell">
      <NvxAppHeader
        :platform="platform"
        workspace
      >
        <template #tabs>
          <NvxWorkspaceTabBar />
        </template>
      </NvxAppHeader>
      <main
        :ref="setWorkspaceContentElement"
        class="workspace-window-shell__content"
      >
        <RouterView v-slot="{ Component, route: renderedRoute }">
          <KeepAlive include="SshTerminalView,DesktopView,FileWorkspaceView,SftpView">
            <component
              :is="Component"
              :key="renderedRoute.path"
            />
          </KeepAlive>
        </RouterView>
        <NvxWorkspaceTabPlaceholder />
      </main>
    </div>
  </NvxWindowFrame>
</template>

<style scoped>
.workspace-window-shell {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  height: 100%;
  min-width: 0;
  min-height: 0;
}

.workspace-window-shell__content {
  position: relative;
  flex: 1 1 auto;
  width: 100%;
  min-width: 0;
  min-height: 0;
  overflow: auto;
}

</style>
