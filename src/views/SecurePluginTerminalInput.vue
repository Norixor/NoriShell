<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { Terminal } from "lucide-vue-next";
import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";
import NvxPluginApprovalPolicy from "../components/plugins/NvxPluginApprovalPolicy.vue";

import { NvxButton, NvxInlineNotice } from "../components/ui";
import {
  decidePluginTerminalInput,
  getPluginTerminalInput,
} from "../core-api/client";
import type {
  PluginTerminalInputDecision,
  PluginTerminalInputProposal,
} from "../core-api/generated/core-api";

const { t } = useI18n();
const approval = ref<PluginTerminalInputProposal | null>(null);
const loading = ref(true);
const pending = ref(false);
const failed = ref(false);
const outcome = ref<"approved" | "rejected" | null>(null);
const policy = ref<"once" | "always">("once");
const expiry = ref<"fifteenMinutes" | "oneHour" | "twentyFourHours" | "unlimited">("unlimited");
const approvalId = new URLSearchParams(window.location.search).get("approvalId") ?? "";

async function load() {
  try {
    const nextApproval = await getPluginTerminalInput(approvalId);
    policy.value = "once";
    expiry.value = "unlimited";
    approval.value = nextApproval;
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
}

async function decide(decision: PluginTerminalInputDecision) {
  if (!approval.value || pending.value) return;
  pending.value = true;
  try {
    const response = await decidePluginTerminalInput({
      approvalId: approval.value.approvalId,
      decision,
      expectedStateVersion: approval.value.stateVersion,
      policy: decision === "approve" ? policy.value : "once",
      expiry: decision === "approve" ? expiry.value : "unlimited",
    });
    outcome.value = response.decision === "approve" ? "approved" : "rejected";
    window.setTimeout(() => void getCurrentWindow().close(), 700);
  } catch {
    failed.value = true;
  } finally {
    pending.value = false;
  }
}

onMounted(() => void load());
</script>

<template>
  <NvxSecureWindow
    class="secure-terminal-input"
    :title="t('plugins.pendingInput.dialogTitle')"
    :description="t('plugins.pendingInput.dialogDescription')"
    :icon="Terminal"
  >
    <p
      v-if="loading"
      role="status"
    >
      {{ t("plugins.loading") }}
    </p>
    <NvxInlineNotice
      v-else-if="failed"
      tone="error"
      :title="t('plugins.hostApproval.failed')"
    />
    <NvxInlineNotice
      v-else-if="outcome"
      :title="t(`plugins.hostApproval.${outcome}`)"
    />
    <template v-else-if="approval">
      <h2>{{ t("plugins.pendingInput.payload") }}</h2>
      <pre class="secure-terminal-input__payload"><code>{{ approval.payload }}{{ approval.appendEnter ? '↵' : '' }}</code></pre>
      <dl class="secure-facts">
        <div><dt>{{ t("plugins.pendingInput.plugin") }}</dt><dd>{{ approval.pluginName }}</dd></div>
        <div><dt>{{ t("plugins.pendingInput.appendEnter") }}</dt><dd>{{ t(`plugins.hostApproval.values.${approval.appendEnter ? "yes" : "no"}`) }}</dd></div>
        <div><dt>{{ t("plugins.pendingInput.publisher") }}</dt><dd>{{ approval.publisher }}</dd></div>
        <div><dt>SHA-256</dt><dd><code>{{ approval.packageSha256 }}</code></dd></div>
        <div><dt>{{ t("plugins.pendingInput.host") }}</dt><dd>{{ approval.hostLabel ?? t("plugins.pendingInput.quickConnect") }}</dd></div>
        <div><dt>{{ t("plugins.pendingInput.endpoint") }}</dt><dd>{{ approval.endpoint }}</dd></div>
      </dl>
      <NvxInlineNotice tone="warning">
        {{ t("plugins.pendingInput.warning") }}
      </NvxInlineNotice>
      <NvxPluginApprovalPolicy
        v-model="policy"
        v-model:model-expiry="expiry"
        :disabled="pending"
        :remember-policy="approval.rememberPolicy"
        risk="terminal"
      />
    </template>
    <template
      v-if="approval && !loading && !failed && !outcome"
      #actions
    >
      <NvxButton
        variant="secondary"
        :disabled="pending"
        @click="decide('reject')"
      >
        {{ t("plugins.pendingInput.reject") }}
      </NvxButton>
      <NvxButton
        :loading="pending"
        @click="decide('approve')"
      >
        {{ policy === "always" ? t("plugins.approvalPolicy.approveAlways") : t("window.approveOnce") }}
      </NvxButton>
    </template>
  </NvxSecureWindow>
</template>

<style scoped>
.secure-terminal-input__payload { max-height: 160px; margin: 0; padding: var(--nvx-space-2); overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); }
</style>
