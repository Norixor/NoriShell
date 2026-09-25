<script setup lang="ts">
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import appIconUrl from "../../assets/branding/norishell-app-icon.png";
import { useAppUpdateStore } from "../../stores/appUpdate";
import { NvxButton, NvxDialog, NvxInlineNotice, NvxStatusLabel } from "../ui";

const PROJECT_PAGE = "https://github.com/Norixor/NoriShell";
const RELEASES_PAGE = "https://github.com/Norixor/NoriShell/releases";

const { t } = useI18n();
const updates = useAppUpdateStore();
const packagedVersion = ref<string | null>(null);
const currentVersion = computed(() => updates.currentVersion ?? packagedVersion.value);
const status = computed(() => updates.status);
const latestVersion = computed(() => updates.latestVersion);
const confirmOpen = ref(false);
const forceConfirmOpen = ref(false);
const openFailed = ref(false);
let mounted = false;

const tone = computed<"success" | "warning" | "danger" | "neutral">(() => {
  if (status.value === "upToDate") return "success";
  if (status.value === "updateAvailable") return "warning";
  if (status.value === "failed") return "danger";
  return "neutral";
});

const statusText = computed(() => {
  if (status.value === "checking") return t("releases.checking");
  if (status.value === "updateAvailable") {
    return t("releases.status.updateAvailable", { version: latestVersion.value ?? "" });
  }
  return t(`releases.status.${status.value}`);
});

async function loadVersion() {
  try {
    const version = await getVersion();
    if (mounted) packagedVersion.value = version;
  } catch {
    // The update action still reads the authoritative packaged version in Core.
  }
}

async function checkForUpdates() {
  openFailed.value = false;
  await updates.checkForUpdates();
}

async function installUpdate() {
  confirmOpen.value = false;
  await updates.installUpdate();
}

async function forceInstallUpdate() {
  forceConfirmOpen.value = false;
  await updates.installUpdate(true);
}

async function openReleases() {
  openFailed.value = false;
  try {
    // The destination is intentionally fixed instead of accepting release HTML URLs.
    await openUrl(RELEASES_PAGE);
  } catch {
    if (mounted) openFailed.value = true;
  }
}

async function openProject() {
  openFailed.value = false;
  try {
    await openUrl(PROJECT_PAGE);
  } catch {
    if (mounted) openFailed.value = true;
  }
}

onMounted(() => {
  mounted = true;
  void loadVersion();
});
onBeforeUnmount(() => {
  mounted = false;
});
</script>

<template>
  <section
    class="about-settings"
    aria-labelledby="about-settings-title"
  >
    <header class="about-settings__hero">
      <img
        class="about-settings__icon"
        :src="appIconUrl"
        alt=""
        width="80"
        height="80"
      >
      <div>
        <h2 id="about-settings-title">
          NoriShell
        </h2>
        <p>{{ t('releases.aboutDescription') }}</p>
        <span class="about-settings__version">
          {{ currentVersion ? t('releases.currentVersion', { version: currentVersion }) : t('releases.versionUnavailable') }}
        </span>
      </div>
    </header>

    <div class="about-settings__rows">
      <div class="about-settings__row">
        <div class="about-settings__copy">
          <strong>{{ t('releases.githubProject') }}</strong>
          <span>{{ t('releases.githubUrl') }}</span>
        </div>
        <NvxButton
          variant="secondary"
          size="sm"
          @click="openProject"
        >
          {{ t('releases.openProject') }}
        </NvxButton>
      </div>

      <div class="about-settings__row">
        <div class="about-settings__copy">
          <strong>{{ t('releases.title') }}</strong>
          <span>{{ t('releases.description') }}</span>
          <NvxStatusLabel :tone="tone">
            {{ statusText }}
          </NvxStatusLabel>
          <span v-if="status === 'updateAvailable' && !updates.supportsAutoInstall">
            {{ t(latestVersion?.includes('-') ? 'releases.install.prereleaseManual' : 'releases.install.portableManual') }}
          </span>
          <span v-if="updates.installStatus === 'checking'">{{ t('releases.install.checking') }}</span>
          <span v-if="updates.installStatus === 'downloading'">
            {{ t('releases.install.downloading', { progress: updates.progressPercent === null ? '…' : `${updates.progressPercent}%` }) }}
          </span>
          <span v-if="updates.installStatus === 'installing'">{{ t('releases.install.installing') }}</span>
          <NvxInlineNotice
            v-if="updates.installStatus === 'resourcesActive'"
            tone="warning"
          >
            {{ t('releases.install.resourcesActive') }}
          </NvxInlineNotice>
          <NvxInlineNotice
            v-if="updates.installStatus === 'failed'"
            tone="error"
          >
            {{ t('releases.install.failed') }}
          </NvxInlineNotice>
          <NvxInlineNotice
            v-if="updates.installStatus === 'cancelled'"
            tone="info"
          >
            {{ t('releases.install.cancelled') }}
          </NvxInlineNotice>
          <NvxInlineNotice
            v-if="updates.installStatus === 'restartRequired'"
            tone="warning"
          >
            {{ t('releases.install.restartRequired') }}
          </NvxInlineNotice>
          <NvxInlineNotice
            v-if="updates.installStatus === 'restartNeeded'"
            tone="warning"
          >
            {{ t('releases.install.restartNeeded') }}
          </NvxInlineNotice>
        </div>
        <div class="about-settings__actions">
          <NvxButton
            v-if="status === 'updateAvailable' && updates.supportsAutoInstall && updates.installStatus === 'resourcesActive'"
            variant="danger"
            size="sm"
            @click="forceConfirmOpen = true"
          >
            {{ t('releases.install.forceAction') }}
          </NvxButton>
          <NvxButton
            v-if="updates.installStatus === 'restartNeeded' || updates.installStatus === 'restartRequired'"
            variant="primary"
            size="sm"
            @click="updates.restartApp()"
          >
            {{ t('releases.install.restartAction') }}
          </NvxButton>
          <NvxButton
            v-if="status === 'updateAvailable' && updates.supportsAutoInstall"
            variant="primary"
            size="sm"
            :disabled="updates.installStatus === 'checking' || updates.installStatus === 'downloading' || updates.installStatus === 'installing' || updates.installStatus === 'restartNeeded' || updates.installStatus === 'restartRequired'"
            @click="confirmOpen = true"
          >
            {{ t('releases.install.action') }}
          </NvxButton>
          <NvxButton
            v-if="status === 'updateAvailable'"
            variant="secondary"
            size="sm"
            @click="openReleases"
          >
            {{ t('releases.openGithub') }}
          </NvxButton>
          <NvxButton
            variant="secondary"
            size="sm"
            :loading="status === 'checking'"
            :disabled="status === 'checking' || updates.installStatus === 'downloading' || updates.installStatus === 'installing'"
            @click="checkForUpdates"
          >
            {{ t('releases.check') }}
          </NvxButton>
        </div>
      </div>
    </div>
    <NvxInlineNotice
      v-if="openFailed"
      tone="error"
    >
      {{ t('releases.openLinkFailed') }}
    </NvxInlineNotice>
    <NvxDialog
      v-model="confirmOpen"
      :title="t('releases.install.confirmTitle')"
      :description="t('releases.install.confirmDescription')"
      :close-label="t('releases.install.cancel')"
      size="md"
    >
      <p>{{ t('releases.install.confirmBody') }}</p>
      <template #actions>
        <NvxButton
          variant="secondary"
          @click="confirmOpen = false"
        >
          {{ t('releases.install.cancel') }}
        </NvxButton>
        <NvxButton
          variant="primary"
          @click="installUpdate"
        >
          {{ t('releases.install.action') }}
        </NvxButton>
      </template>
    </NvxDialog>
    <NvxDialog
      v-model="forceConfirmOpen"
      :title="t('releases.install.forceConfirmTitle')"
      :description="t('releases.install.forceConfirmDescription')"
      :close-label="t('releases.install.cancel')"
      size="md"
    >
      <p>{{ t('releases.install.forceConfirmBody') }}</p>
      <template #actions>
        <NvxButton
          variant="secondary"
          @click="forceConfirmOpen = false"
        >
          {{ t('releases.install.cancel') }}
        </NvxButton>
        <NvxButton
          variant="danger"
          @click="forceInstallUpdate"
        >
          {{ t('releases.install.forceAction') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.about-settings {
  display: grid;
  gap: var(--nvx-space-5);
  min-width: 0;
}

.about-settings__hero {
  display: flex;
  gap: var(--nvx-space-5);
  align-items: center;
  padding-bottom: var(--nvx-space-5);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.about-settings__icon {
  width: 80px;
  height: 80px;
  border-radius: 18px;
  box-shadow: var(--nvx-shadow-toast);
}

.about-settings__hero > div,
.about-settings__copy {
  min-width: 0;
}

.about-settings h2,
.about-settings p {
  margin: 0;
}

.about-settings h2 {
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-xl);
  line-height: var(--nvx-line-height-xl);
}

.about-settings p,
.about-settings__copy span {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
  line-height: 1.5;
}

.about-settings p {
  margin-top: var(--nvx-space-1);
}

.about-settings__version {
  display: inline-block;
  margin-top: var(--nvx-space-2);
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
  font-variant-numeric: tabular-nums;
}

.about-settings__rows {
  display: grid;
  overflow: hidden;
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-lg);
  background: var(--nvx-color-bg-surface);
}

.about-settings__row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: var(--nvx-space-5);
  align-items: center;
  min-height: 82px;
  padding: var(--nvx-space-4) var(--nvx-space-5);
}

.about-settings__row + .about-settings__row {
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.about-settings__copy {
  display: grid;
  gap: var(--nvx-space-1);
}

.about-settings__copy strong {
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-semibold);
}

.about-settings__actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--nvx-space-2);
  justify-content: flex-end;
}

@media (max-width: 760px) {
  .about-settings__row {
    grid-template-columns: 1fr;
  }

  .about-settings__actions {
    justify-content: flex-start;
  }
}
</style>
