<script setup lang="ts">
import { ChevronDown, Columns2, Download, Eye, FolderOpen, FolderPlus, Pencil, Rows2, Server, Trash2, Upload, X } from "lucide-vue-next";
import { useI18n } from "vue-i18n";

import { NvxButton, NvxIcon } from "../ui";
import { usePopoverMenu } from "../ui/usePopoverMenu";
import NvxPluginContributionSlot from "../terminal/NvxPluginContributionSlot.vue";

defineProps<{
  paneId: string;
  kind: "local" | "remote";
  ready: boolean;
  hasSelection: boolean;
  singleSelection: boolean;
  pending: boolean;
  showHidden: boolean;
  foldersFirst: boolean;
  canSplitHorizontal: boolean;
  canSplitVertical: boolean;
  canClose: boolean;
}>();

const emit = defineEmits<{
  chooseFolder: [];
  upload: [];
  download: [];
  mkdir: [];
  rename: [];
  delete: [];
  toggleShowHidden: [];
  toggleFoldersFirst: [];
  split: [direction: "horizontal" | "vertical"];
  addRemote: [];
  close: [];
}>();

const { t } = useI18n();
const { rootRef, triggerRef, viewportPanelRef, viewportPanelStyle, open, closeMenu, toggleMenu, handleMenuKeyDown } = usePopoverMenu();

function run(action: "chooseFolder" | "upload" | "download" | "mkdir" | "rename" | "delete" | "close") {
  if (action === "chooseFolder") emit("chooseFolder");
  else if (action === "upload") emit("upload");
  else if (action === "download") emit("download");
  else if (action === "mkdir") emit("mkdir");
  else if (action === "rename") emit("rename");
  else if (action === "delete") emit("delete");
  else emit("close");
  closeMenu(true);
}

function runSplit(direction: "horizontal" | "vertical") {
  emit("split", direction);
  closeMenu(true);
}

function addRemote() {
  emit("addRemote");
  closeMenu(true);
}

function toggleBrowserPreference(action: "toggleShowHidden" | "toggleFoldersFirst") {
  if (action === "toggleShowHidden") emit("toggleShowHidden");
  else emit("toggleFoldersFirst");
  closeMenu(true);
}

</script>

<template>
  <div
    :ref="rootRef"
    class="sftp-pane-actions-menu"
  >
    <NvxButton
      :ref="triggerRef"
      size="sm"
      variant="ghost"
      :aria-label="t('sftp.moreActions')"
      aria-haspopup="menu"
      :aria-expanded="open"
      @click="toggleMenu"
    >
      {{ t("sftp.more") }}<NvxIcon
        :icon="ChevronDown"
        :size="16"
      />
    </NvxButton>

    <Teleport to="body">
      <div
        v-if="open"
        :ref="viewportPanelRef"
        class="sftp-pane-actions-menu__popover"
        :data-pane-id="paneId"
        :style="viewportPanelStyle"
        role="menu"
        :aria-label="t('sftp.moreActions')"
        @keydown="handleMenuKeyDown"
      >
        <NvxPluginContributionSlot
          extension-slot="sftpContextMenu"
          menu
          :instance-key="paneId"
        />
        <button
          class="sftp-pane-actions-menu__item"
          type="button"
          role="menuitemcheckbox"
          :aria-checked="showHidden"
          :disabled="pending"
          @click="toggleBrowserPreference('toggleShowHidden')"
        >
          <NvxIcon
            :icon="Eye"
            :size="16"
          />{{ t("sftpSettings.showHidden") }}
        </button>
        <button
          class="sftp-pane-actions-menu__item"
          type="button"
          role="menuitemcheckbox"
          :aria-checked="foldersFirst"
          :disabled="pending"
          @click="toggleBrowserPreference('toggleFoldersFirst')"
        >
          <NvxIcon
            :icon="FolderOpen"
            :size="16"
          />{{ t("sftpSettings.foldersFirst") }}
        </button>
        <div
          class="sftp-pane-actions-menu__separator"
          role="separator"
        />
        <button
          v-if="kind === 'local'"
          class="sftp-pane-actions-menu__item"
          type="button"
          role="menuitem"
          :disabled="pending"
          @click="run('chooseFolder')"
        >
          <NvxIcon
            :icon="FolderOpen"
            :size="16"
          />{{ t("sftp.changeLocalFolder") }}
        </button>
        <button
          v-if="kind === 'local'"
          class="sftp-pane-actions-menu__item"
          type="button"
          role="menuitem"
          :disabled="pending || (!canSplitHorizontal && !canSplitVertical)"
          @click="addRemote"
        >
          <NvxIcon
            :icon="Server"
            :size="16"
          />{{ t("sftp.addRemotePane") }}
        </button>
        <template v-else>
          <button
            class="sftp-pane-actions-menu__item"
            type="button"
            role="menuitem"
            :disabled="pending || !ready"
            @click="run('upload')"
          >
            <NvxIcon
              :icon="Upload"
              :size="16"
            />{{ t("sftp.chooseUpload") }}
          </button>
          <button
            class="sftp-pane-actions-menu__item"
            type="button"
            role="menuitem"
            :disabled="pending || !singleSelection"
            @click="run('download')"
          >
            <NvxIcon
              :icon="Download"
              :size="16"
            />{{ t("sftp.download") }}
          </button>
          <div
            class="sftp-pane-actions-menu__separator"
            role="separator"
          />
          <button
            class="sftp-pane-actions-menu__item"
            type="button"
            role="menuitem"
            :disabled="pending || !ready"
            @click="run('mkdir')"
          >
            <NvxIcon
              :icon="FolderPlus"
              :size="16"
            />{{ t("sftp.newFolder") }}
          </button>
          <button
            class="sftp-pane-actions-menu__item"
            type="button"
            role="menuitem"
            :disabled="pending || !singleSelection"
            @click="run('rename')"
          >
            <NvxIcon
              :icon="Pencil"
              :size="16"
            />{{ t("sftp.rename") }}
          </button>
          <button
            class="sftp-pane-actions-menu__item sftp-pane-actions-menu__item--danger"
            type="button"
            role="menuitem"
            :disabled="pending || !hasSelection"
            @click="run('delete')"
          >
            <NvxIcon
              :icon="Trash2"
              :size="16"
            />{{ t("sftp.delete") }}
          </button>
        </template>
        <div
          class="sftp-pane-actions-menu__separator"
          role="separator"
        />
        <button
          class="sftp-pane-actions-menu__item"
          type="button"
          role="menuitem"
          :disabled="pending || !canSplitHorizontal"
          @click="runSplit('horizontal')"
        >
          <NvxIcon
            :icon="Columns2"
            :size="16"
          />{{ t("sftp.splitRight") }}
        </button>
        <button
          class="sftp-pane-actions-menu__item"
          type="button"
          role="menuitem"
          :disabled="pending || !canSplitVertical"
          @click="runSplit('vertical')"
        >
          <NvxIcon
            :icon="Rows2"
            :size="16"
          />{{ t("sftp.splitDown") }}
        </button>
        <button
          class="sftp-pane-actions-menu__item sftp-pane-actions-menu__item--danger"
          type="button"
          role="menuitem"
          :disabled="pending || !canClose"
          @click="run('close')"
        >
          <NvxIcon
            :icon="X"
            :size="16"
          />{{ t("sftp.closePane") }}
        </button>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.sftp-pane-actions-menu { position:relative; display:inline-flex; flex:none; }
.sftp-pane-actions-menu__popover { position:fixed; z-index:var(--nvx-z-popover); display:grid; box-sizing:border-box; width:min(224px,calc(100vw - 24px)); max-height:calc(100dvh - 16px); overflow-y:auto; padding:2px; border:var(--nvx-border-width) solid var(--nvx-color-border-strong); border-radius:var(--nvx-radius-md); background:var(--nvx-color-bg-surface); box-shadow:var(--nvx-shadow-overlay); }
.sftp-pane-actions-menu__item { display:flex; gap:var(--nvx-space-2); align-items:center; width:100%; min-height:36px; padding:0 var(--nvx-space-3); border:0; border-radius:var(--nvx-radius-sm); background:transparent; color:var(--nvx-color-text-primary); font:inherit; text-align:left; }
.sftp-pane-actions-menu__item:hover:not(:disabled),.sftp-pane-actions-menu__item:focus-visible { background:var(--nvx-color-bg-hover); outline:none; }
.sftp-pane-actions-menu__item--danger { color:var(--nvx-color-danger); }
.sftp-pane-actions-menu__item:disabled { opacity:.45; }
.sftp-pane-actions-menu__separator { height:1px; margin:2px var(--nvx-space-2); background:var(--nvx-color-border-subtle); }
</style>
