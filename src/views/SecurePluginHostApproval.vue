<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { Server } from "lucide-vue-next";
import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";
import NvxPluginApprovalPolicy from "../components/plugins/NvxPluginApprovalPolicy.vue";

import { NvxButton, NvxInlineNotice } from "../components/ui";
import {
  decidePluginHostApproval,
  getPluginHostApproval,
} from "../core-api/client";
import type {
  PluginApprovalDecision,
  PluginHostApprovalSummary,
} from "../core-api/generated/core-api";

const { t } = useI18n();
const approval = ref<PluginHostApprovalSummary | null>(null);
const loading = ref(true);
const pending = ref(false);
const failed = ref(false);
const outcome = ref<"approved" | "rejected" | null>(null);
const policy = ref<"once" | "always">("once");
const expiry = ref<"fifteenMinutes" | "oneHour" | "twentyFourHours" | "unlimited">("unlimited");
const approvalId = new URLSearchParams(window.location.search).get("approvalId") ?? "";
const mutationFacts = computed(() => {
  const patch = approval.value?.mutationPatch;
  if (!patch) return [];
  return Object.entries(patch)
    .filter(([field, value]) => value !== null && !(field === "clearUsername" && value === false))
    .map(([field, value]) => ({ field, value: typeof value === "boolean" ? t(`plugins.hostApproval.values.${value ? "yes" : "no"}`) : String(value) }));
});

async function load() {
  try {
    const nextApproval = await getPluginHostApproval(approvalId);
    policy.value = "once";
    expiry.value = "unlimited";
    approval.value = nextApproval;
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
}

async function decide(decision: PluginApprovalDecision) {
  if (!approval.value || pending.value) return;
  pending.value = true;
  try {
    const response = await decidePluginHostApproval({
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
    class="secure-approval"
    :title="t('plugins.hostApproval.title')"
    :description="t('plugins.hostApproval.description')"
    :icon="Server"
  >
    <p
      v-if="loading"
      role="status"
    >
      {{ t("plugins.hostApproval.loading") }}
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
      <section v-if="approval.kind === 'mutation'">
        <h2>{{ t("plugins.hostApproval.mutation") }}</h2>
        <dl class="secure-facts">
          <div
            v-for="fact in mutationFacts"
            :key="fact.field"
          >
            <dt>{{ t(`plugins.hostApproval.fields.${fact.field}`) }}</dt><dd>{{ fact.value }}</dd>
          </div>
        </dl>
      </section>
      <section v-else>
        <h2>{{ t("plugins.hostApproval.session") }}</h2>
        <p>{{ approval.sessionKind ? t(`plugins.hostApproval.sessionKinds.${approval.sessionKind}`) : "" }}</p>
      </section>
      <dl class="secure-facts">
        <div><dt>{{ t("plugins.hostApproval.plugin") }}</dt><dd>{{ approval.pluginName }}</dd></div>
        <div><dt>{{ t("plugins.hostApproval.host") }}</dt><dd>{{ approval.hostLabel }}</dd></div>
        <div><dt>{{ t("plugins.hostApproval.endpoint") }}</dt><dd>{{ approval.endpoint }}</dd></div>
        <div><dt>{{ t("plugins.hostApproval.reason") }}</dt><dd>{{ approval.reason }}</dd></div>
      </dl>
      <NvxPluginApprovalPolicy
        v-model="policy"
        v-model:model-expiry="expiry"
        :disabled="pending"
        :remember-policy="approval.rememberPolicy"
        risk="host"
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
        {{ t("plugins.hostApproval.reject") }}
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
.secure-approval section:not(.nvx-inline-notice) { display: grid; gap: var(--nvx-space-3); }
</style>
