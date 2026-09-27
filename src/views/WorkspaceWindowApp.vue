<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";

import { NvxAppHeader, NvxWindowFrame, NvxWorkspaceTabBar } from "../components/layout";
import NvxWorkspaceIncomingSkeleton from "../components/layout/NvxWorkspaceIncomingSkeleton.vue";
import { NvxTips } from "../components/ui";
import { useUiStore } from "../stores/ui";
import { useTipsStore } from "../stores/tips";
import { i18n } from "../locales";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import { startWorkspaceTabWindowUi } from "../workspace-tab-window-ui";
import { visibleIncomingWorkspaceTabs } from "../workspace-tab-transfer";

const ui = useUiStore();
const router = useRouter();
const workspaceTabs = useWorkspaceTabsStore();
const tips = useTipsStore();
const incomingSkeletonFull = ref(false);
watch(visibleIncomingWorkspaceTabs, (count, previous) => {
  if (count && !previous) {
    incomingSkeletonFull.value = !(
      workspaceTabs.terminalTabs.length + workspaceTabs.pageTabs.length
      + workspaceTabs.fileTabs.length + workspaceTabs.desktopTabs.length
    );
  } else if (!count) {
    incomingSkeletonFull.value = false;
  }
}, { flush: "sync" });
let stopWorkspaceTabWindows: (() => void) | null = null;
let disposed = false;

ui.applyPreferences();
void ui.setUiZoom(ui.uiZoom, false);
onMounted(() => {
  void startWorkspaceTabWindowUi(workspaceTabs, router).then((stop) => {
    if (disposed) stop(); else stopWorkspaceTabWindows = stop;
  }).catch(() => {
    tips.show({ scope: "workspace-tab-windows", tone: "error", title: i18n.global.t("workspaceTabs.unavailable") });
  });
});
onBeforeUnmount(() => {
  disposed = true;
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
        class="workspace-window-shell__content"
      >
        <RouterView v-slot="{ Component, route }">
          <KeepAlive include="SshTerminalView,DesktopView,FileWorkspaceView,SftpView">
            <component
              :is="Component"
              :key="route.path"
            />
          </KeepAlive>
        </RouterView>
        <Transition name="workspace-incoming-fade">
          <NvxWorkspaceIncomingSkeleton
            v-if="visibleIncomingWorkspaceTabs"
            :full="incomingSkeletonFull"
          />
        </Transition>
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
