<script setup lang="ts">
import { registerToolWindowExit, type ToolWindowExitController } from "../tool-window-exit";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { onMounted, onBeforeUnmount, ref } from "vue";
import { useI18n } from "vue-i18n";
import NvxHostEditor from "../components/hosts/NvxHostEditor.vue";
import NvxDesktopProfileEditor from "../components/desktop/NvxDesktopProfileEditor.vue";
import NvxSftpFileWindow from "../components/sftp/NvxSftpFileWindow.vue";
import { NvxInlineNotice, NvxTips } from "../components/ui";
import { closeToolWindow, notifyToolWindowChanged, type ToolTarget } from "../tool-windows";
const { t } = useI18n();
const target = ref<ToolTarget | null>(null), failed = ref(false);
const editor = ref<{ requestClose(): Promise<boolean>; busy?: boolean } | null>(null);
let exitController: ToolWindowExitController | undefined;
let unlisten: (() => void) | undefined, disposed = false, closing = false;
async function close() {
  if (closing) return;
  closing = true;
  try { if (!editor.value || await editor.value.requestClose()) await closeToolWindow(); }
  catch { failed.value = true; }
  finally { closing = false; }
}
async function closeApproved() { try { await closeToolWindow(); } catch { failed.value = true; } }
async function saved() {
  try { await notifyToolWindowChanged(); await closeToolWindow(); }
  catch { failed.value = true; }
}
onMounted(async () => {
  try {
    unlisten = await getCurrentWindow().onCloseRequested((event) => { event.preventDefault(); void close(); });
    if (disposed) { unlisten(); return; }
    exitController = await registerToolWindowExit({ requestClose: () => editor.value?.requestClose() ?? Promise.resolve(true), isBusy: () => !target.value || !!editor.value?.busy, close: closeToolWindow, onError: () => { failed.value = true; } });
    if (disposed) { exitController.dispose(); return; }
    target.value = await invoke<ToolTarget>("tool_window_get");
  } catch { failed.value = true; }
});
onBeforeUnmount(() => { disposed = true; unlisten?.(); exitController?.dispose(); });
</script>
<template>
  <main
    class="tool-window"
    data-plugin-protected
    data-theme-protected
  >
    <header
      class="tool-window__title"
      data-tauri-drag-region
    >
      {{ target?.title ?? 'NoriShell' }}
    </header>
    <NvxInlineNotice
      v-if="failed"
      tone="error"
      :title="t('errors.tray.actionUnavailable')"
    />
    <NvxTips />
    <div
      class="tool-window__body"
      :class="{ 'tool-window__body--sftp-file': target?.kind === 'sftpFile' }"
    >
      <NvxHostEditor
        v-if="target?.kind === 'hostEditor'"
        ref="editor"
        :host-id="target.hostId ?? undefined"
        :initial-section="target.initialSection"
        @close-cancelled="exitController?.keepOpen()"
        @saved="saved"
        @cancel="closeApproved"
      />
      <NvxDesktopProfileEditor
        v-else-if="target?.kind === 'desktopEditor'"
        ref="editor"
        :profile-id="target.profileId ?? undefined"
        @close-cancelled="exitController?.keepOpen()"
        @saved="saved"
        @cancel="close"
      />
      <NvxSftpFileWindow
        v-else-if="target?.kind === 'sftpFile'"
        ref="editor"
        :target="target"
        @close-cancelled="exitController?.keepOpen()"
        @saved="saved"
        @cancel="close"
      />
    </div>
  </main>
</template>
<style scoped>
:global(html:has(#tool-app)), :global(body:has(#tool-app)), :global(#tool-app) { margin: 0; width: 100%; height: 100%; min-width: 0; }
.tool-window { height: 100dvh; display: flex; flex-direction: column; color: var(--nvx-color-text-primary); background: var(--nvx-color-bg-canvas); }
.tool-window__title { padding: 8px 100px; min-height: 36px; text-align: center; border-bottom: 1px solid var(--nvx-color-border); font-weight: 600; overflow-wrap: anywhere; }
.tool-window__body { flex: 1; min-height: 0; overflow: auto; padding: var(--nvx-space-4); }
.tool-window__body--sftp-file { overflow: hidden; padding: 0; }
</style>
