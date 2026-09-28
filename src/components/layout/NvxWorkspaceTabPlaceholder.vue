<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import { pendingWorkspaceTabViewId, workspaceTabViewSummary } from "../../workspace-tab-view-state";
import type { WorkspaceTabKind } from "../../workspace-tab-windows";

// A new Tab's native WebView normally appears within ~0.3–0.6s. The skeleton
// alone carries that wait; text is announced only when the open is slow, so a
// quick open never flashes a caption or interrupts a screen reader.
const SLOW_OPEN_NOTICE_DELAY_MS = 1000;

type SkeletonVariant = "terminal" | "file" | "desktop" | "new" | "settings" | "grid" | "page";

const { t } = useI18n();

const pending = computed(() => {
  const id = pendingWorkspaceTabViewId.value;
  return id ? { id, summary: workspaceTabViewSummary(id) } : null;
});

function variantFor(kind: WorkspaceTabKind | undefined, route: string | undefined): SkeletonVariant {
  if (kind === "terminal") return "terminal";
  if (kind === "file") return "file";
  if (kind === "desktop") return "desktop";
  if (route === "/new") return "new";
  if (route?.startsWith("/settings")) return "settings";
  if (route === "/overview" || route === "/hosts" || route === "/plugins" || route === "/tunnels") return "grid";
  return "page";
}

const variant = computed<SkeletonVariant>(() =>
  variantFor(pending.value?.summary?.kind, pending.value?.summary?.route));

const slow = ref(false);
let slowTimer: ReturnType<typeof setTimeout> | null = null;

function clearSlowTimer() {
  if (slowTimer !== null) clearTimeout(slowTimer);
  slowTimer = null;
}

watch(() => pending.value?.id ?? null, (id) => {
  clearSlowTimer();
  slow.value = false;
  if (!id) return;
  slowTimer = setTimeout(() => {
    slowTimer = null;
    slow.value = true;
  }, SLOW_OPEN_NOTICE_DELAY_MS);
}, { immediate: true });

onBeforeUnmount(clearSlowTimer);

const fileRows = [72, 58, 84, 46, 66, 52, 78, 40, 60];
const desktopRows = [64, 48, 72, 56];
const gridCards = [0, 1, 2, 3, 4, 5];
const settingsNav = [56, 72, 48, 64, 60, 44];
const recentRows = [0, 1, 2];
</script>

<template>
  <!--
    Covers the shell page while a new Tab renders. It appears opaque at once and
    the native Tab WebView is shown on top of it before it is removed, so it never
    fades in or out.
  -->
  <div
    v-if="pending"
    class="nvx-workspace-tab-placeholder"
    :class="`nvx-workspace-tab-placeholder--${variant}`"
    :data-variant="variant"
  >
    <div
      class="nvx-workspace-tab-placeholder__skeleton"
      aria-hidden="true"
      aria-busy="true"
    >
      <!-- Terminal: pane toolbar over the terminal surface, first prompt line with a cursor. -->
      <template v-if="variant === 'terminal'">
        <div class="nvx-tab-skeleton__terminal-toolbar">
          <i
            class="nvx-tab-skeleton__bar"
            style="width: 96px"
          />
          <i
            class="nvx-tab-skeleton__bar"
            style="width: 64px"
          />
        </div>
        <div class="nvx-tab-skeleton__terminal-screen">
          <div class="nvx-tab-skeleton__prompt">
            <i
              class="nvx-tab-skeleton__bar"
              style="width: 7ch"
            />
            <i class="nvx-tab-skeleton__cursor" />
          </div>
        </div>
      </template>

      <!-- File: SFTP top bar, pane header, command bar and an entry list. -->
      <template v-else-if="variant === 'file'">
        <div class="nvx-tab-skeleton__file-topbar">
          <i class="nvx-tab-skeleton__square" />
          <i
            class="nvx-tab-skeleton__bar"
            style="width: 128px"
          />
          <i
            class="nvx-tab-skeleton__pill"
            style="width: 56px"
          />
          <i
            class="nvx-tab-skeleton__pill nvx-tab-skeleton__end"
            style="width: 88px"
          />
        </div>
        <div class="nvx-tab-skeleton__file-pane-header">
          <i class="nvx-tab-skeleton__square" />
          <i
            class="nvx-tab-skeleton__bar"
            style="width: 112px"
          />
          <i
            class="nvx-tab-skeleton__bar nvx-tab-skeleton__end"
            style="width: 176px"
          />
        </div>
        <div class="nvx-tab-skeleton__file-commandbar">
          <i
            v-for="index in 4"
            :key="index"
            class="nvx-tab-skeleton__square nvx-tab-skeleton__square--lg"
          />
          <i
            class="nvx-tab-skeleton__bar"
            style="width: 240px"
          />
        </div>
        <div class="nvx-tab-skeleton__file-list">
          <div
            v-for="(width, index) in fileRows"
            :key="index"
            class="nvx-tab-skeleton__file-row"
          >
            <i class="nvx-tab-skeleton__square" />
            <i
              class="nvx-tab-skeleton__bar"
              :style="{ width: `${width * 2}px` }"
            />
            <i
              class="nvx-tab-skeleton__bar nvx-tab-skeleton__end"
              style="width: 48px"
            />
            <i
              class="nvx-tab-skeleton__bar"
              style="width: 104px"
            />
          </div>
        </div>
      </template>

      <!-- Desktop: profile sidebar beside an empty remote screen frame. -->
      <template v-else-if="variant === 'desktop'">
        <div class="nvx-tab-skeleton__desktop-sidebar">
          <div class="nvx-tab-skeleton__desktop-header">
            <i
              class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--title"
              style="width: 96px"
            />
            <i
              class="nvx-tab-skeleton__pill nvx-tab-skeleton__end"
              style="width: 64px"
            />
          </div>
          <div
            v-for="(width, index) in desktopRows"
            :key="index"
            class="nvx-tab-skeleton__desktop-row"
          >
            <i class="nvx-tab-skeleton__square nvx-tab-skeleton__square--lg" />
            <span class="nvx-tab-skeleton__lines">
              <i
                class="nvx-tab-skeleton__bar"
                :style="{ width: `${width + 40}px` }"
              />
              <i
                class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--thin"
                :style="{ width: `${width}px` }"
              />
            </span>
          </div>
        </div>
        <div class="nvx-tab-skeleton__desktop-stage">
          <i class="nvx-tab-skeleton__desktop-frame" />
        </div>
      </template>

      <!-- New page: heading, two entry cards and the recent-hosts rows. -->
      <template v-else-if="variant === 'new'">
        <div class="nvx-tab-skeleton__new-content">
          <div class="nvx-tab-skeleton__new-header">
            <span class="nvx-tab-skeleton__lines">
              <i
                class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--display"
                style="width: 176px"
              />
              <i
                class="nvx-tab-skeleton__bar"
                style="width: 288px"
              />
            </span>
            <i
              class="nvx-tab-skeleton__pill"
              style="width: 120px"
            />
          </div>
          <div class="nvx-tab-skeleton__new-cards">
            <div
              v-for="index in 2"
              :key="index"
              class="nvx-tab-skeleton__new-card"
            >
              <div class="nvx-tab-skeleton__new-card-heading">
                <i class="nvx-tab-skeleton__square nvx-tab-skeleton__square--icon" />
                <span class="nvx-tab-skeleton__lines">
                  <i
                    class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--title"
                    style="width: 128px"
                  />
                  <i
                    class="nvx-tab-skeleton__bar"
                    style="width: 208px"
                  />
                </span>
              </div>
              <i class="nvx-tab-skeleton__new-preview" />
              <div class="nvx-tab-skeleton__new-actions">
                <i class="nvx-tab-skeleton__pill nvx-tab-skeleton__pill--button" />
                <i class="nvx-tab-skeleton__pill nvx-tab-skeleton__pill--button" />
              </div>
            </div>
          </div>
          <div class="nvx-tab-skeleton__new-recent">
            <i
              class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--title"
              style="width: 112px"
            />
            <div
              v-for="index in recentRows"
              :key="index"
              class="nvx-tab-skeleton__new-recent-row"
            >
              <i class="nvx-tab-skeleton__square" />
              <i
                class="nvx-tab-skeleton__bar"
                style="width: 136px"
              />
              <i
                class="nvx-tab-skeleton__bar"
                style="width: 112px"
              />
              <i
                class="nvx-tab-skeleton__bar nvx-tab-skeleton__end"
                style="width: 96px"
              />
            </div>
          </div>
        </div>
      </template>

      <!-- Settings: section navigation column beside a form column. -->
      <template v-else-if="variant === 'settings'">
        <div class="nvx-tab-skeleton__settings-nav">
          <i
            v-for="(width, index) in settingsNav"
            :key="index"
            class="nvx-tab-skeleton__bar"
            :style="{ width: `${width + 40}px` }"
          />
        </div>
        <div class="nvx-tab-skeleton__settings-content">
          <i
            class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--display"
            style="width: 160px"
          />
          <div
            v-for="index in 4"
            :key="index"
            class="nvx-tab-skeleton__settings-row"
          >
            <span class="nvx-tab-skeleton__lines">
              <i
                class="nvx-tab-skeleton__bar"
                style="width: 144px"
              />
              <i
                class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--thin"
                style="width: 256px"
              />
            </span>
            <i
              class="nvx-tab-skeleton__pill nvx-tab-skeleton__end"
              style="width: 160px"
            />
          </div>
        </div>
      </template>

      <!-- Overview / Hosts / Plugins / Tunnels: page header over a card grid. -->
      <template v-else-if="variant === 'grid'">
        <div class="nvx-tab-skeleton__page-header">
          <span class="nvx-tab-skeleton__lines">
            <i
              class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--display"
              style="width: 160px"
            />
            <i
              class="nvx-tab-skeleton__bar"
              style="width: 272px"
            />
          </span>
          <i
            class="nvx-tab-skeleton__pill"
            style="width: 112px"
          />
        </div>
        <div class="nvx-tab-skeleton__grid">
          <div
            v-for="index in gridCards"
            :key="index"
            class="nvx-tab-skeleton__card"
          >
            <i
              class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--title"
              style="width: 60%"
            />
            <i
              class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--thin"
              style="width: 80%"
            />
            <i
              class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--thin"
              style="width: 45%"
            />
          </div>
        </div>
      </template>

      <!-- Any other page: header and a single content card. -->
      <template v-else>
        <div class="nvx-tab-skeleton__page-header">
          <span class="nvx-tab-skeleton__lines">
            <i
              class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--display"
              style="width: 160px"
            />
            <i
              class="nvx-tab-skeleton__bar"
              style="width: 272px"
            />
          </span>
        </div>
        <div class="nvx-tab-skeleton__card nvx-tab-skeleton__card--wide">
          <i
            class="nvx-tab-skeleton__bar"
            style="width: 40%"
          />
          <i
            class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--thin"
            style="width: 70%"
          />
          <i
            class="nvx-tab-skeleton__bar nvx-tab-skeleton__bar--thin"
            style="width: 55%"
          />
        </div>
      </template>
    </div>

    <!-- Live region exists from the start so the delayed caption is announced once. -->
    <p
      class="nvx-workspace-tab-placeholder__notice"
      :class="{ 'nvx-workspace-tab-placeholder__notice--visible': slow }"
      role="status"
      aria-live="polite"
    >
      {{ slow ? t("workspaceTabs.opening") : "" }}
    </p>
  </div>
</template>

<style scoped>
.nvx-workspace-tab-placeholder {
  --nvx-tab-skeleton-bar: var(--nvx-color-border);
  --nvx-tab-skeleton-bar-opacity: 1;
  position: absolute;
  z-index: 20;
  inset: 0;
  overflow: hidden;
  /* Opaque from its first frame: the shell hides the previous Tab view right after
     this paints, so an entrance fade would let the shell page show through. */
  background: var(--nvx-color-bg-canvas);
}

.nvx-workspace-tab-placeholder--file,
.nvx-workspace-tab-placeholder--settings {
  background: var(--nvx-color-bg-surface);
}

.nvx-workspace-tab-placeholder--terminal {
  --nvx-tab-skeleton-bar: var(--nvx-color-terminal-muted);
  --nvx-tab-skeleton-bar-opacity: 0.3;
}

.nvx-workspace-tab-placeholder__skeleton {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  animation: nvx-workspace-tab-placeholder-breathe 1.6s ease-in-out infinite alternate;
}

/* Shared primitives */
.nvx-tab-skeleton__bar,
.nvx-tab-skeleton__pill,
.nvx-tab-skeleton__square {
  display: block;
  flex: 0 0 auto;
  max-width: 100%;
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-tab-skeleton-bar);
  opacity: var(--nvx-tab-skeleton-bar-opacity);
}

.nvx-tab-skeleton__bar {
  height: 10px;
}

.nvx-tab-skeleton__bar--thin {
  height: 8px;
}

.nvx-tab-skeleton__bar--title {
  height: 14px;
}

.nvx-tab-skeleton__bar--display {
  height: 20px;
}

.nvx-tab-skeleton__pill {
  height: var(--nvx-control-height-sm);
  border-radius: var(--nvx-radius-md);
}

.nvx-tab-skeleton__pill--button {
  flex: 1 1 0;
  height: var(--nvx-control-height-md);
}

.nvx-tab-skeleton__square {
  width: 16px;
  height: 16px;
}

.nvx-tab-skeleton__square--lg {
  width: 24px;
  height: 24px;
}

.nvx-tab-skeleton__square--icon {
  width: 64px;
  height: 64px;
  border-radius: 10px;
  background: var(--nvx-color-accent-soft);
}

.nvx-tab-skeleton__end {
  margin-inline-start: auto;
}

.nvx-tab-skeleton__lines {
  display: grid;
  gap: var(--nvx-space-2);
  min-width: 0;
}

/* Terminal */
.nvx-tab-skeleton__terminal-toolbar {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  min-height: 36px;
  padding: 0 var(--nvx-space-2);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  background: var(--nvx-color-bg-canvas);
}

.nvx-tab-skeleton__terminal-toolbar .nvx-tab-skeleton__bar {
  --nvx-tab-skeleton-bar: var(--nvx-color-border);
  --nvx-tab-skeleton-bar-opacity: 1;
}

.nvx-tab-skeleton__terminal-screen {
  flex: 1 1 auto;
  padding: var(--nvx-space-2) var(--nvx-space-3);
  background: var(--nvx-color-terminal-bg);
  font-family: var(--nvx-font-mono);
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
}

.nvx-tab-skeleton__prompt {
  display: flex;
  gap: 1ch;
  align-items: center;
  height: var(--nvx-line-height-sm);
}

.nvx-tab-skeleton__cursor {
  display: block;
  width: 1ch;
  height: 1.1em;
  background: var(--nvx-color-terminal-cursor);
  animation: nvx-workspace-tab-placeholder-blink 1.1s steps(1, end) infinite;
}

/* File */
.nvx-tab-skeleton__file-topbar,
.nvx-tab-skeleton__file-pane-header,
.nvx-tab-skeleton__file-commandbar,
.nvx-tab-skeleton__file-row {
  display: flex;
  flex: 0 0 auto;
  gap: var(--nvx-space-3);
  align-items: center;
}

.nvx-tab-skeleton__file-topbar {
  min-height: 56px;
  padding: var(--nvx-space-2) var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.nvx-tab-skeleton__file-pane-header {
  min-height: 50px;
  padding: var(--nvx-space-2) var(--nvx-space-3);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.nvx-tab-skeleton__file-commandbar {
  gap: var(--nvx-space-2);
  min-height: 48px;
  padding: var(--nvx-space-1) var(--nvx-space-3);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.nvx-tab-skeleton__file-commandbar .nvx-tab-skeleton__bar {
  margin-inline-start: var(--nvx-space-2);
}

.nvx-tab-skeleton__file-list {
  display: grid;
  align-content: start;
  padding-block: var(--nvx-space-1);
}

.nvx-tab-skeleton__file-row {
  height: 32px;
  padding-inline: var(--nvx-space-3);
}

/* Desktop */
.nvx-workspace-tab-placeholder--desktop .nvx-workspace-tab-placeholder__skeleton {
  flex-direction: row;
}

.nvx-tab-skeleton__desktop-sidebar {
  display: grid;
  flex: 0 0 var(--nvx-layout-inspector-width);
  align-content: start;
  gap: var(--nvx-space-2);
  padding: var(--nvx-space-4);
  border-inline-end: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
}

.nvx-tab-skeleton__desktop-header {
  display: flex;
  align-items: center;
  min-height: var(--nvx-control-height-sm);
  margin-bottom: var(--nvx-space-3);
}

.nvx-tab-skeleton__desktop-row {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  padding: var(--nvx-space-2) var(--nvx-space-1);
}

.nvx-tab-skeleton__desktop-stage {
  display: grid;
  flex: 1 1 auto;
  place-items: center;
  min-width: 0;
}

.nvx-tab-skeleton__desktop-frame {
  display: block;
  width: 48%;
  aspect-ratio: 16 / 10;
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-subtle);
}

/* New page */
.nvx-tab-skeleton__new-content {
  width: 100%;
  max-width: 1320px;
  margin: 0 auto;
  padding: 36px 40px 50px;
}

.nvx-tab-skeleton__new-header {
  display: flex;
  gap: var(--nvx-space-6);
  align-items: center;
  justify-content: space-between;
  min-height: 62px;
  margin-bottom: 28px;
}

.nvx-tab-skeleton__new-cards {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 20px;
}

.nvx-tab-skeleton__new-card {
  display: flex;
  flex-direction: column;
  min-width: 0;
  padding: var(--nvx-space-6);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: 8px;
  background: var(--nvx-color-bg-surface);
}

.nvx-tab-skeleton__new-card-heading {
  display: flex;
  gap: var(--nvx-space-6);
  align-items: center;
  min-height: 72px;
  margin-bottom: 18px;
}

.nvx-tab-skeleton__new-preview {
  display: block;
  height: 278px;
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-subtle);
}

.nvx-tab-skeleton__new-actions {
  display: flex;
  gap: var(--nvx-space-3);
  margin-top: var(--nvx-space-5);
}

.nvx-tab-skeleton__new-recent {
  display: grid;
  gap: var(--nvx-space-3);
  margin-top: var(--nvx-space-8);
}

.nvx-tab-skeleton__new-recent-row {
  display: flex;
  gap: var(--nvx-space-6);
  align-items: center;
  height: 44px;
  padding-inline: var(--nvx-space-3);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

/* Settings */
.nvx-workspace-tab-placeholder--settings .nvx-workspace-tab-placeholder__skeleton {
  flex-direction: row;
}

.nvx-tab-skeleton__settings-nav {
  display: grid;
  flex: 0 0 236px;
  align-content: start;
  gap: var(--nvx-space-5);
  padding: var(--nvx-space-6) var(--nvx-space-5);
  border-inline-end: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-canvas);
}

.nvx-tab-skeleton__settings-content {
  display: grid;
  flex: 1 1 auto;
  align-content: start;
  gap: var(--nvx-space-4);
  min-width: 0;
  max-width: 760px;
  padding: var(--nvx-space-6) var(--nvx-space-8);
}

.nvx-tab-skeleton__settings-row {
  display: flex;
  gap: var(--nvx-space-6);
  align-items: center;
  min-height: 56px;
  padding-block: var(--nvx-space-2);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

/* Grid and generic pages */
.nvx-tab-skeleton__page-header {
  display: flex;
  gap: var(--nvx-space-6);
  align-items: flex-start;
  justify-content: space-between;
  padding: var(--nvx-space-6) var(--nvx-space-6) 0;
  margin-bottom: var(--nvx-space-6);
}

.nvx-tab-skeleton__grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: var(--nvx-space-4);
  padding: 0 var(--nvx-space-6) var(--nvx-space-6);
}

.nvx-tab-skeleton__card {
  display: grid;
  gap: var(--nvx-space-3);
  min-height: 132px;
  padding: var(--nvx-space-4);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
}

.nvx-tab-skeleton__card--wide {
  margin: 0 var(--nvx-space-6);
}

/* Delayed caption for slow opens */
.nvx-workspace-tab-placeholder__notice {
  position: absolute;
  inset-inline: 0;
  bottom: var(--nvx-space-8);
  width: max-content;
  max-width: calc(100% - var(--nvx-space-8));
  margin: 0 auto;
  padding: var(--nvx-space-1) var(--nvx-space-3);
  border-radius: var(--nvx-radius-md);
  /* Same surface as the current variant so the caption reads over skeleton rows. */
  background: inherit;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
  text-align: center;
  opacity: 0;
  transition: opacity var(--nvx-motion-overlay) ease-out;
  pointer-events: none;
}

.nvx-workspace-tab-placeholder__notice--visible {
  opacity: 1;
}

@keyframes nvx-workspace-tab-placeholder-breathe {
  from { opacity: 1; }
  to { opacity: 0.6; }
}

@keyframes nvx-workspace-tab-placeholder-blink {
  0%, 100% { opacity: 1; }
  50% { opacity: 0; }
}

/* Static skeleton only. */
@media (prefers-reduced-motion: reduce) {
  .nvx-workspace-tab-placeholder__skeleton,
  .nvx-tab-skeleton__cursor {
    animation: none;
  }

  .nvx-workspace-tab-placeholder__skeleton {
    opacity: 0.8;
  }
}
</style>
