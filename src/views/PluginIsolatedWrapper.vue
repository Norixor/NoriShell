<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { convertFileSrc } from "@tauri-apps/api/core";

import NvxButton from "../components/ui/NvxButton.vue";
import { getPluginIsolatedSurfaceContent, invokePluginIsolatedBridge } from "../core-api/client";
import type { PluginIsolatedPendingAction } from "../core-api/generated/core-api";
import { PluginIsolatedBridge } from "../plugin-isolated-bridge";

const { t, locale } = useI18n();
const frame = ref<HTMLIFrameElement | null>(null);
const documentUrl = ref("");
const unavailable = ref(false);
const failureCode = ref("");
const pending = ref<PluginIsolatedPendingAction | null>(null);
const approving = ref(false);
const params = new URLSearchParams(window.location.search);
const surfaceId = params.get("surfaceId") ?? "";
const channelNonce = params.get("channelNonce") ?? "";
let bridge: PluginIsolatedBridge | null = null;
let loaded = false;
let appearance: MutationObserver | null = null;

const pendingLabel = computed(() => {
  if ((pending.value?.operation === "filePick" || pending.value?.operation === "sftpOpen")) return t("plugins.isolated.fileAccess");
  if (pending.value?.operation === "processStart") return t("plugins.isolated.processAccess");
  if (pending.value?.operation === "credential") return t("plugins.isolated.credentialAccess");
  if (pending.value?.operation === "remoteExecStart") return t("plugins.isolated.remoteAccess");
  return t("plugins.isolated.networkAccess");
});

function fail(code: string) {
  failureCode.value = code;
  bridge?.dispose();
  documentUrl.value = "";
  unavailable.value = true;
}

function environment() {
  bridge?.environment(locale.value, document.documentElement.dataset.theme ?? "light");
}

function frameLoaded() {
  if (loaded || !bridge || !frame.value?.contentWindow) {
    fail("surface-load");
    return;
  }
  loaded = true;
  const channel = new MessageChannel();
  bridge.attach(channel.port1);
  // The opaque sandbox origin requires '*'; the target is the exact iframe WindowProxy.
  frame.value.contentWindow.postMessage({ type: "norishell.bridge.ready", protocol: 13,
    locale: locale.value, theme: document.documentElement.dataset.theme ?? "light" }, "*", [channel.port2]);
}

async function approve(event: MouseEvent) {
  if (!event.isTrusted || !pending.value || approving.value) return;
  const id = pending.value.pendingId;
  approving.value = true;
  try { await bridge?.approve(id); }
  finally { approving.value = false; }
}

async function cancel(event: MouseEvent) {
  if (!event.isTrusted || !pending.value || approving.value) return;
  await bridge?.cancel(pending.value.pendingId);
}

function dispose() { bridge?.dispose(); }
watch(locale, environment);

onMounted(async () => {
  window.addEventListener("pagehide", dispose);
  appearance = new MutationObserver(environment);
  appearance.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  if (!surfaceId || !channelNonce) { fail("surface-address"); return; }
  try {
    const content = await getPluginIsolatedSurfaceContent(surfaceId, channelNonce);
    if (content.surfaceId !== surfaceId) { fail("surface-identity"); return; }
    bridge = new PluginIsolatedBridge(content.nextSequence, {
      invoke: (sequence, action) => invokePluginIsolatedBridge({ surfaceId, channelNonce, sequence, action }),
      pending: (value) => { pending.value = value; },
      unavailable: () => fail("surface-bridge"),
    });
    documentUrl.value = convertFileSrc(content.documentToken, "norishell-plugin");
  } catch { fail("surface-content"); }
});

onBeforeUnmount(() => {
  dispose();
  appearance?.disconnect();
  window.removeEventListener("pagehide", dispose);
});
</script>

<template>
  <section class="plugin-isolated-wrapper">
    <p
      v-if="unavailable"
      class="plugin-isolated-wrapper__unavailable"
      role="status"
    >
      {{ t("plugins.isolated.unavailable") }}
      <small>{{ t("plugins.isolated.errorCode", { code: failureCode }) }}</small>
    </p>
    <iframe
      v-else-if="documentUrl"
      ref="frame"
      :src="documentUrl"
      sandbox="allow-scripts"
      referrerpolicy="no-referrer"
      :title="t('plugins.isolated.title')"
      @load="frameLoaded"
    />
    <div
      v-if="pending || approving"
      class="plugin-isolated-wrapper__approval"
      data-plugin-protected
    >
      <div>
        <strong>{{ approving ? t("plugins.isolated.reviewing") : pendingLabel }}</strong>
        <p>{{ t("plugins.isolated.explanation") }}</p>
      </div>
      <template v-if="pending">
        <NvxButton
          variant="ghost"
          size="sm"
          @click="cancel"
        >
          {{ t("common.cancel") }}
        </NvxButton>
        <NvxButton
          size="sm"
          @click="approve"
        >
          {{ t("plugins.isolated.continue") }}
        </NvxButton>
      </template>
    </div>
  </section>
</template>

<style scoped>
.plugin-isolated-wrapper { display: flex; flex-direction: column; width: 100%; height: 100%; }
.plugin-isolated-wrapper > iframe { flex: 1; min-height: 0; width: 100%; border: 0; }
.plugin-isolated-wrapper__unavailable { margin: auto; padding: 24px; color: var(--nvx-color-text-secondary); }
.plugin-isolated-wrapper__unavailable small { display: block; margin-top: 8px; }
.plugin-isolated-wrapper__approval { display: flex; align-items: center; gap: 12px; padding: 12px 16px; border-top: 1px solid var(--nvx-color-border); background: var(--nvx-color-bg-surface); }
.plugin-isolated-wrapper__approval > div { flex: 1; min-width: 0; }
.plugin-isolated-wrapper__approval strong { font-size: 13px; }
.plugin-isolated-wrapper__approval p { margin: 4px 0 0; font-size: 12px; color: var(--nvx-color-text-secondary); }
</style>
