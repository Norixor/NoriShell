<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { ArrowRight, Server, ShieldAlert } from "lucide-vue-next";
import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";
import NvxPluginApprovalPolicy from "../components/plugins/NvxPluginApprovalPolicy.vue";

import { NvxButton, NvxField, NvxIcon, NvxInlineNotice, NvxInput } from "../components/ui";
import { decidePluginRemoteApproval, getPluginRemoteApproval, submitPluginCredentialInput } from "../core-api/client";
import type { PluginApprovalDecision, PluginRemoteApprovalPrompt } from "../core-api/generated/core-api";
import { MIN_NEW_SECRET_PASSWORD_CHARACTERS, passwordCharacterCount } from "../password-policy";

const { t, locale } = useI18n();
const prompt = ref<PluginRemoteApprovalPrompt | null>(null);
const loading = ref(true);
const pending = ref(false);
const failed = ref(false);
const policy = ref<"once" | "always">("once");
const expiry = ref<"fifteenMinutes" | "oneHour" | "twentyFourHours" | "unlimited">("unlimited");
const credentialSecret = ref("");
const credentialConfirmation = ref("");
const credentialByteLimit = computed(() => content.value?.kind === "vaultAccess" ? 65_536 : 4_096);
const credentialTooLong = computed(() => new TextEncoder().encode(credentialSecret.value).byteLength > credentialByteLimit.value);
const confirmationTooLong = computed(() => new TextEncoder().encode(credentialConfirmation.value).byteLength > 65_536);
const vaultPasswordTooShort = computed(() => createsVault.value && !!credentialSecret.value
  && passwordCharacterCount(credentialSecret.value) < MIN_NEW_SECRET_PASSWORD_CHARACTERS);
const vaultConfirmationMismatch = computed(() => createsVault.value && !!credentialConfirmation.value
  && credentialSecret.value !== credentialConfirmation.value);
const credentialValid = computed(() => {
  const bytes = new TextEncoder().encode(credentialSecret.value).length;
  return bytes > 0 && bytes <= credentialByteLimit.value && (content.value?.kind !== "vaultAccess"
    || !content.value.create || (passwordCharacterCount(credentialSecret.value) >= MIN_NEW_SECRET_PASSWORD_CHARACTERS
      && !confirmationTooLong.value && credentialSecret.value === credentialConfirmation.value));
});
const approvalId = new URLSearchParams(window.location.search).get("approvalId") ?? "";
const content = computed(() => prompt.value?.content);
const takesSecret = computed(() => content.value?.kind === "credential" || content.value?.kind === "vaultAccess");
const createsVault = computed(() => content.value?.kind === "vaultAccess" && content.value.create);
const isSftpAccess = computed(() => content.value?.kind === "access"
  && (content.value.operation === "sftpRead" || content.value.operation === "sftpWrite"));
const accessRisk = computed<"network" | "file" | "local" | "remote" | "sftp" | "serial">(() => {
  if (content.value?.kind !== "access") return "network";
  if (content.value.operation === "serialAccess") return "serial";
  if (content.value.operation === "localExecute") return "local";
  if (content.value.operation === "remoteExecute") return "remote";
  if (isSftpAccess.value) return "sftp";
  if (content.value.operation === "fileAccess") return "file";
  return "network";
});
const title = computed(() => {
  if (content.value?.kind === "vaultAccess") return t(content.value.create ? "plugins.remoteApproval.vaultCreateTitle" : "plugins.remoteApproval.vaultUnlockTitle");
  if (content.value?.kind !== "access") return t(`plugins.remoteApproval.${content.value?.kind ?? "execute"}Title`);
  if (content.value.operation === "serialAccess") return t("plugins.remoteApproval.serialAccessTitle");
  if (content.value.operation === "localExecute") return t("plugins.remoteApproval.localExecuteTitle");
  if (content.value.operation === "remoteExecute") return t("plugins.remoteApproval.executeTitle");
  if (isSftpAccess.value) return t("plugins.remoteApproval.sftpAccessTitle");
  if (content.value.operation === "fileAccess") return t("plugins.remoteApproval.fileAccessTitle");
  return t("plugins.remoteApproval.accessTitle");
});
const connectionContext = computed(() => content.value?.kind === "credential"
  ? t("plugins.remoteApproval.credentialDestination")
  : content.value?.kind === "access" && content.value.operation === "serialAccess"
  ? t("plugins.remoteApproval.serialContext")
  : content.value?.kind === "access" && content.value.operation === "localExecute"
  ? t("plugins.remoteApproval.localExecutionContext")
  : content.value?.kind === "access" && content.value.operation === "remoteExecute"
    ? t("plugins.remoteApproval.independentExecutionContext")
    : isSftpAccess.value ? t("plugins.remoteApproval.sftpConnectionContext")
      : content.value?.kind === "access" && content.value.operation === "fileAccess"
        ? t("plugins.remoteApproval.fileAccessContext")
        : content.value?.kind === "access" ? t("plugins.remoteApproval.networkAccessContext")
      : t("plugins.remoteApproval.currentConnection"));
const requestedBy = computed(() => content.value?.kind === "vaultAccess"
  ? t("plugins.remoteApproval.requestedVaultBy", { plugin: prompt.value?.pluginName ?? "" })
  : content.value?.kind === "credential"
  ? t("plugins.remoteApproval.requestedCredentialBy", { plugin: prompt.value?.pluginName ?? "" })
  : content.value?.kind === "access" && content.value.operation === "localExecute"
  ? t("plugins.remoteApproval.requestedLocalBy", { plugin: prompt.value?.pluginName ?? "" })
  : isSftpAccess.value ? t("plugins.remoteApproval.requestedSftpBy", { plugin: prompt.value?.pluginName ?? "" })
    : content.value?.kind === "access" && content.value.operation === "fileAccess"
      ? t("plugins.remoteApproval.requestedFileBy", { plugin: prompt.value?.pluginName ?? "" })
      : content.value?.kind === "access" ? t("plugins.remoteApproval.requestedOperationBy", { plugin: prompt.value?.pluginName ?? "" })
    : t("plugins.remoteApproval.requestedBy", { plugin: prompt.value?.pluginName ?? "" }));
const accessWarning = computed(() => {
  if (content.value?.kind !== "access") return t("plugins.remoteApproval.accessWarning");
  if (content.value.operation === "serialAccess") return t("plugins.remoteApproval.serialAccessWarning");
  if (content.value.operation === "localExecute") return t("plugins.remoteApproval.localExecuteWarning");
  if (content.value.operation === "remoteExecute") return t("plugins.remoteApproval.independentExecuteWarning");
  if (isSftpAccess.value) return t("plugins.remoteApproval.sftpAccessWarning");
  if (content.value.operation === "fileAccess") return t("plugins.remoteApproval.fileAccessWarning");
  return t("plugins.remoteApproval.accessWarning");
});
const rememberPolicy = computed(() => prompt.value?.rememberPolicy ?? "unavailable");

const forwardRoute = computed(() => {
  const value = content.value;
  if (!value || (value.kind !== "forwardStart" && value.kind !== "forwardStop")) return null;
  const rule = value.rule;
  const remote = rule.kind === "remote";
  const bind = remote ? `${rule.remoteBindAddress}:${rule.remoteListenPort}` : `${rule.localBindAddress}:${rule.localListenPort}`;
  const target = rule.kind === "remote" ? `${rule.localTargetHost}:${rule.localTargetPort}`
    : rule.kind === "local" ? `${rule.remoteTargetHost}:${rule.remoteTargetPort}` : t("plugins.remoteApproval.socksTarget");
  return {
    kind: t(`plugins.remoteApproval.forward${rule.kind[0]?.toUpperCase()}${rule.kind.slice(1)}`),
    bind,
    target,
    bindLabel: t(remote ? "plugins.remoteApproval.remoteListen" : "plugins.remoteApproval.localListen"),
    targetLabel: t(remote ? "plugins.remoteApproval.localTarget" : "plugins.remoteApproval.remoteTarget"),
    actualBind: value.kind === "forwardStop" && value.actualBind ? `${value.actualBind.address}:${value.actualBind.port}` : null,
  };
});

async function decide(decision: PluginApprovalDecision) {
  if (!prompt.value || pending.value) return;
  if (decision === "approve" && takesSecret.value && !credentialValid.value) return;
  pending.value = true;
  try {
    if (decision === "approve" && takesSecret.value) {
      const secret = credentialSecret.value;
      const confirmation = createsVault.value ? credentialConfirmation.value : undefined;
      credentialSecret.value = "";
      credentialConfirmation.value = "";
      await submitPluginCredentialInput({
        approvalId: prompt.value.approvalId,
        expectedStateVersion: prompt.value.stateVersion,
        secret,
        ...(confirmation !== undefined ? { confirmation } : {}),
      });
    } else await decidePluginRemoteApproval({
      approvalId: prompt.value.approvalId,
      expectedStateVersion: prompt.value.stateVersion,
      decision,
      policy: decision === "approve" ? policy.value : "once",
      expiry: decision === "approve" ? expiry.value : "unlimited",
    });
    void getCurrentWindow().close().catch(() => undefined);
  } catch {
    failed.value = true;
  } finally {
    credentialSecret.value = "";
    credentialConfirmation.value = "";
    pending.value = false;
  }
}

onBeforeUnmount(() => { credentialSecret.value = ""; credentialConfirmation.value = ""; });

onMounted(async () => {
  try {
    const nextPrompt = await getPluginRemoteApproval(approvalId);
    policy.value = "once";
    expiry.value = "unlimited";
    prompt.value = nextPrompt;
    if (prompt.value.locale === "en" || prompt.value.locale === "zh-CN") locale.value = prompt.value.locale;
    document.documentElement.lang = prompt.value.locale;
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <NvxSecureWindow
    class="secure-remote"
    :title="title"
    :description="prompt ? requestedBy : t('plugins.remoteApproval.loading')"
    :icon="ShieldAlert"
  >
    <div class="secure-remote__body">
      <p
        v-if="loading"
        role="status"
      >
        {{ t("plugins.remoteApproval.loading") }}
      </p>
      <NvxInlineNotice
        v-else-if="failed"
        tone="error"
        :title="t('plugins.remoteApproval.failed')"
      />
      <template v-else-if="prompt && content">
        <template v-if="content.kind === 'execute'">
          <p>{{ content.reason }}</p>
          <h2>{{ t("plugins.remoteApproval.command") }}</h2>
          <pre>{{ content.command }}</pre>
          <template v-if="content.stdin !== null">
            <h2>{{ t("plugins.remoteApproval.stdin") }}</h2>
            <pre>{{ content.stdin || t("plugins.remoteApproval.emptyInput") }}</pre>
          </template>
        </template>
        <template v-else-if="content.kind === 'credential' || content.kind === 'vaultAccess'">
          <template v-if="content.kind === 'credential'">
            <h2>{{ content.label }}</h2>
            <p>{{ t("plugins.remoteApproval.credentialInjection") }} · <code>{{ content.target.injection.kind === "bearer" ? "Authorization: Bearer" : content.target.injection.name }}</code></p>
          </template>
          <p v-else>
            {{ t(createsVault ? "plugins.remoteApproval.vaultCreateHint" : "plugins.remoteApproval.vaultUnlockHint") }}
          </p>
          <NvxField
            for-id="plugin-credential-secret"
            :label="t(content.kind === 'credential' ? 'plugins.remoteApproval.credentialValue' : 'plugins.remoteApproval.vaultPassword')"
            :hint="createsVault ? t('plugins.remoteApproval.vaultCreateHint') : undefined"
            :error="credentialTooLong ? t('plugins.remoteApproval.passwordTooLong') : vaultPasswordTooShort ? t('sshTerminal.vaultPasswordTooShort') : undefined"
          >
            <NvxInput
              id="plugin-credential-secret"
              v-model="credentialSecret"
              type="password"
              autocomplete="new-password"
              :disabled="pending"
              :invalid="vaultPasswordTooShort || credentialTooLong"
              @keydown.enter.prevent="decide('approve')"
            />
          </NvxField>
          <NvxField
            v-if="createsVault"
            for-id="plugin-vault-confirmation"
            :label="t('plugins.remoteApproval.vaultConfirmation')"
            :hint="t('plugins.sshSync.confirmPasswordHint')"
            :error="confirmationTooLong ? t('plugins.remoteApproval.passwordTooLong') : vaultConfirmationMismatch ? t('sshTerminal.vaultPasswordMismatch') : undefined"
          >
            <NvxInput
              id="plugin-vault-confirmation"
              v-model="credentialConfirmation"
              type="password"
              autocomplete="new-password"
              :disabled="pending"
              :invalid="vaultConfirmationMismatch || confirmationTooLong"
              @keydown.enter.prevent="decide('approve')"
            />
          </NvxField>
        </template>
        <template v-else-if="content.kind === 'access'">
          <h2>{{ t(`plugins.approvalPolicy.operations.${content.operation}`) }}</h2>
          <p class="secure-remote__reason">
            {{ content.reason }}
          </p>
          <h2>{{ t("plugins.remoteApproval.accessDetails") }}</h2>
          <pre>{{ content.details }}</pre>
        </template>
        <template v-else-if="forwardRoute">
          <div class="secure-remote__route">
            <span class="secure-remote__kind">{{ forwardRoute.kind }}</span>
            <div class="secure-remote__endpoint">
              <span>{{ forwardRoute.bindLabel }}</span><code>{{ forwardRoute.bind }}</code>
            </div>
            <NvxIcon
              class="secure-remote__arrow"
              :icon="ArrowRight"
              :size="20"
            />
            <div class="secure-remote__endpoint">
              <span>{{ forwardRoute.targetLabel }}</span><code>{{ forwardRoute.target }}</code>
            </div>
          </div>
          <p
            v-if="forwardRoute.actualBind"
            class="secure-remote__actual"
          >
            {{ t('plugins.remoteApproval.actualBind') }} · <code>{{ forwardRoute.actualBind }}</code>
          </p>
          <p class="secure-remote__reason">
            {{ content.kind === 'forwardStart' || content.kind === 'forwardStop' ? content.reason : '' }}
          </p>
        </template>
        <div
          v-if="content.kind !== 'vaultAccess'"
          class="secure-remote__connection"
        >
          <NvxIcon
            :icon="Server"
            :size="20"
          />
          <div><strong>{{ prompt.hostLabel }}</strong><span>{{ connectionContext }}</span><code>{{ prompt.endpoint }}</code></div>
        </div>
        <NvxInlineNotice
          tone="warning"
          :title="content.kind === 'vaultAccess' ? t('plugins.remoteApproval.vaultPasswordWarning') : content.kind === 'credential' ? t('plugins.remoteApproval.credentialWarning') : content.kind === 'access' ? accessWarning : t(content.kind === 'execute' ? 'plugins.remoteApproval.executionWarning' : 'plugins.remoteApproval.forwardWarning')"
        />
        <NvxPluginApprovalPolicy
          v-if="!takesSecret"
          v-model="policy"
          v-model:model-expiry="expiry"
          :disabled="pending"
          :remember-policy="rememberPolicy"
          :risk="content.kind === 'access' ? accessRisk : 'remote'"
        />
      </template>
    </div>
    <template #actions>
      <NvxButton
        variant="secondary"
        :disabled="pending"
        @click="prompt && !failed ? decide('reject') : getCurrentWindow().close()"
      >
        {{ t("plugins.remoteApproval.reject") }}
      </NvxButton>
      <NvxButton
        v-if="prompt && !failed"
        :variant="content?.kind === 'execute' ? 'danger' : 'primary'"
        :disabled="loading || (takesSecret && !credentialValid)"
        :loading="pending"
        @click="decide('approve')"
      >
        {{ policy === "always" ? t("plugins.approvalPolicy.approveAlways") : t("window.approveOnce") }}
      </NvxButton>
    </template>
  </NvxSecureWindow>
</template>

<style scoped>
.secure-remote__body { display: grid; gap: var(--nvx-space-3); min-width: 0; }
.secure-remote pre { max-height: 160px; overflow: auto; margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; padding: var(--nvx-space-2); border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); font-family: var(--nvx-font-mono); }
.secure-remote__route { display: grid; grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr); column-gap: var(--nvx-space-4); row-gap: var(--nvx-space-2); align-items: center; padding-block: var(--nvx-space-2); }
.secure-remote__kind { grid-column: 1 / -1; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.secure-remote__endpoint { display: grid; gap: var(--nvx-space-1); min-width: 0; }
.secure-remote__endpoint > span { font-size: var(--nvx-font-size-xs); color: var(--nvx-color-text-secondary); }
.secure-remote__endpoint > code { font-size: var(--nvx-font-size-md); line-height: var(--nvx-line-height-md); overflow-wrap: anywhere; }
.secure-remote__arrow { color: var(--nvx-color-text-tertiary); }
.secure-remote__connection { display: flex; align-items: center; gap: var(--nvx-space-3); border-top: var(--nvx-border-width) solid var(--nvx-color-border); padding-top: var(--nvx-space-3); }
.secure-remote__connection > svg { flex: none; color: var(--nvx-color-text-secondary); }
.secure-remote__connection > div { min-width: 0; }
.secure-remote__connection span { margin-left: var(--nvx-space-2); font-size: var(--nvx-font-size-xs); color: var(--nvx-color-text-secondary); }
.secure-remote__connection code { display: block; overflow-wrap: anywhere; color: var(--nvx-color-text-secondary); }
.secure-remote__reason, .secure-remote__actual { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
</style>
