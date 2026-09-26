<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Eye, EyeOff } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";

import {
  NvxButton,
  NvxCheckbox,
  NvxField,
  NvxIcon,
  NvxInlineNotice,
  NvxInput,
  NvxIconButton,
} from "../components/ui";
import { decideSshSyncSecurePrompt, getSshSyncSecurePrompt } from "../core-api/client";
import type { SshSyncSecureDifference, SshSyncSecurePrompt } from "../core-api/generated/core-api";
import { MIN_NEW_SECRET_PASSWORD_CHARACTERS, passwordCharacterCount } from "../password-policy";

const { t, locale } = useI18n();
const promptId = new URLSearchParams(window.location.search).get("promptId") ?? "";
const prompt = ref<SshSyncSecurePrompt | null>(null);
const selectedHosts = ref<string[]>([]);
const selectedDesktops = ref<string[]>([]);
const selectedCredentials = ref<string[]>([]);
const selectedDirection = ref<"keepLocal" | "useRemote" | null>(null);
const vaultPassword = ref("");
const vaultPasswordConfirmation = ref("");
const passwordVisible = ref(false);
const loading = ref(true);
const pending = ref(false);
const failed = ref(false);
const completed = ref(false);
const cancelled = ref(false);
let closeTimer: number | undefined;

const asksForPassword = computed(() => [
  "createRecoveryPassword",
  "recoverExistingKey",
  "createLocalVault",
  "unlockSynchronizedVault",
  "recoverSynchronizedKey",
].includes(prompt.value?.kind ?? ""));
const createsLocalVault = computed(() => prompt.value?.kind === "createLocalVault");
const isRemoteReset = computed(() => prompt.value?.kind === "resetRemote");
const isDirectionChoice = computed(() => prompt.value?.kind === "chooseSyncDirection");
const isDataApply = computed(() => prompt.value?.kind === "approveDataApply");
const isDataReview = computed(() => prompt.value?.kind === "reviewDataChoices");
const selectedReview = computed(() => {
  if (!selectedDirection.value) return null;
  const source = selectedDirection.value === "keepLocal" ? "local" : "remote";
  return prompt.value?.dataReviewChoices.find((choice) => choice.source === source) ?? null;
});
const reviewSides = computed(() => {
  if (!prompt.value || !selectedReview.value) return [];
  return [
    { name: "local", before: { hostCount: prompt.value.hostCount, credentialCount: prompt.value.credentialCount, desktopProfileCount: prompt.value.desktopProfileCount }, after: selectedReview.value.local },
    { name: "remote", before: { hostCount: prompt.value.remoteHostCount, credentialCount: prompt.value.remoteCredentialCount, desktopProfileCount: prompt.value.remoteDesktopProfileCount }, after: selectedReview.value.remote },
  ];
});
const isDataApplyUploaded = computed(() => isDataApply.value && prompt.value?.dataApplyUploaded === true);
const isConflictChoice = computed(() => prompt.value?.kind === "resolveConflicts");
const isMergedDeletion = computed(() => prompt.value?.kind === "approveMergedDeletion");
const requiresDirectionChoice = computed(() => ["resolveConflicts", "chooseSyncDirection", "reviewDataChoices"].includes(prompt.value?.kind ?? ""));
const createsPassword = computed(() => createsLocalVault.value || prompt.value?.kind === "createRecoveryPassword");
const passwordTooShort = computed(() => createsPassword.value && !!vaultPassword.value
  && passwordCharacterCount(vaultPassword.value) < MIN_NEW_SECRET_PASSWORD_CHARACTERS);
const existingPasswordTooShort = computed(() => asksForPassword.value && !createsPassword.value
  && !!vaultPassword.value && new TextEncoder().encode(vaultPassword.value).byteLength < 8);
const confirmationMismatch = computed(() => createsLocalVault.value && !!vaultPasswordConfirmation.value
  && vaultPassword.value !== vaultPasswordConfirmation.value);
const passwordValid = computed(() => !asksForPassword.value || (
  (createsPassword.value
    ? passwordCharacterCount(vaultPassword.value) >= MIN_NEW_SECRET_PASSWORD_CHARACTERS
    : new TextEncoder().encode(vaultPassword.value).byteLength >= 8)
  && (!createsLocalVault.value || vaultPassword.value === vaultPasswordConfirmation.value)
));

function clearVaultPasswords() {
  vaultPassword.value = "";
  vaultPasswordConfirmation.value = "";
}

function formatTime(value: number | null) {
  if (value === null || !Number.isFinite(value) || Number.isNaN(new Date(value).getTime())) {
    return t("plugins.sshSync.timeUnavailable");
  }
  return new Intl.DateTimeFormat(locale.value, {
    dateStyle: "medium",
    timeStyle: "medium",
  }).format(new Date(value));
}

function differenceSummary(difference: SshSyncSecureDifference, side: "local" | "remote") {
  if (difference.change === (side === "local" ? "remoteOnly" : "localOnly")) {
    return t("plugins.sshSync.absent");
  }
  if (difference.kind === "encryptedSecret") {
    return t(difference.change === "changed" ? "plugins.sshSync.secretChanged" : "plugins.sshSync.secretHidden");
  }
  if (difference.kind === "preferences") {
    return t("plugins.sshSync.preferencesHidden");
  }
  return difference[side === "local" ? "localSummary" : "remoteSummary"] || t("plugins.sshSync.present");
}

async function closeWindow() {
  clearVaultPasswords();
  try {
    await getCurrentWindow().close();
  } catch {
    failed.value = true;
  }
}

async function cancel() {
  if (pending.value) return;
  if (!prompt.value || failed.value || completed.value) {
    await closeWindow();
  } else {
    await decide("cancel");
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key !== "Escape") return;
  event.preventDefault();
  void cancel();
}

function hostSelected(hostId: string) {
  return selectedHosts.value.includes(hostId);
}

function updateHost(hostId: string, selected: boolean) {
  selectedHosts.value = selected
    ? [...new Set([...selectedHosts.value, hostId])]
    : selectedHosts.value.filter((value) => value !== hostId);
  if (!selected) {
    const credentialIds = new Set(
      prompt.value?.hosts.find((host) => host.hostId === hostId)?.credentials
        .map((credential) => credential.credentialRefId) ?? [],
    );
    const retained = new Set([
      ...(prompt.value?.hosts.filter((host) => selectedHosts.value.includes(host.hostId)).flatMap((host) => host.credentials.map((c) => c.credentialRefId)) ?? []),
      ...(prompt.value?.desktopProfiles.filter((profile) => selectedDesktops.value.includes(profile.profileId)).flatMap((profile) => profile.credentials.map((c) => c.credentialRefId)) ?? []),
    ]);
    selectedCredentials.value = selectedCredentials.value.filter((value) => !credentialIds.has(value) || retained.has(value));
  }
}

function credentialSelected(credentialRefId: string) {
  return selectedCredentials.value.includes(credentialRefId);
}

function updateCredential(hostId: string, credentialRefId: string, selected: boolean) {
  if (selected && !hostSelected(hostId)) updateHost(hostId, true);
  selectedCredentials.value = selected
    ? [...new Set([...selectedCredentials.value, credentialRefId])]
    : selectedCredentials.value.filter((value) => value !== credentialRefId);
}

function updateDesktop(profileId: string, selected: boolean) {
  selectedDesktops.value = selected ? [...new Set([...selectedDesktops.value, profileId])] : selectedDesktops.value.filter((id) => id !== profileId);
  if (!selected) {
    const removed = prompt.value?.desktopProfiles.find((p) => p.profileId === profileId)?.credentials.map((c) => c.credentialRefId) ?? [];
    const retained = new Set([
      ...(prompt.value?.hosts.filter((h) => selectedHosts.value.includes(h.hostId)).flatMap((h) => h.credentials.map((c) => c.credentialRefId)) ?? []),
      ...(prompt.value?.desktopProfiles.filter((p) => selectedDesktops.value.includes(p.profileId)).flatMap((p) => p.credentials.map((c) => c.credentialRefId)) ?? []),
    ]);
    selectedCredentials.value = selectedCredentials.value.filter((id) => !removed.includes(id) || retained.has(id));
  }
}

function updateDesktopCredential(profileId: string, credentialId: string, selected: boolean) {
  if (selected) updateDesktop(profileId, true);
  selectedCredentials.value = selected ? [...new Set([...selectedCredentials.value, credentialId])] : selectedCredentials.value.filter((id) => id !== credentialId);
}

async function load() {
  try {
    const next = await getSshSyncSecurePrompt(promptId);
    prompt.value = next;
    selectedDirection.value = null;
    if (next.kind === "selectBackup") {
      selectedDesktops.value = next.desktopProfiles.map((profile) => profile.profileId);
      selectedHosts.value = next.hosts.map((host) => host.hostId);
      selectedCredentials.value = next.hosts.flatMap((host) => (
        host.credentials
          .filter((credential) => !credential.machineBound)
          .map((credential) => credential.credentialRefId)
      ));
      selectedCredentials.value = [...new Set([...selectedCredentials.value, ...next.desktopProfiles.flatMap((profile) => profile.credentials.map((credential) => credential.credentialRefId))])];
    }
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
}

async function decide(decision: "approve" | "cancel" | "keepLocal" | "useRemote" | "applyMerged") {
  if (!prompt.value || pending.value) return;
  if (isDataReview.value && decision !== "cancel" && !selectedReview.value) return;
  pending.value = true;
  const submitPassword = decision === "approve" && asksForPassword.value
    ? vaultPassword.value
    : null;
  const submitPasswordConfirmation = decision === "approve" && createsLocalVault.value
    ? vaultPasswordConfirmation.value
    : undefined;
  clearVaultPasswords();
  try {
    const response = await decideSshSyncSecurePrompt({
      promptId: prompt.value.promptId,
      decision,
      selectedDesktopProfileIds: decision === "approve" ? selectedDesktops.value : [],
      selectedHostIds: decision === "approve" ? selectedHosts.value : [],
      selectedCredentialRefIds: decision === "approve" ? selectedCredentials.value : [],
      vaultPassword: submitPassword,
      vaultPasswordConfirmation: submitPasswordConfirmation,
    });
    if (!response.accepted) throw new Error("Secure decision rejected");
    cancelled.value = decision === "cancel";
    completed.value = true;
    closeTimer = window.setTimeout(() => void closeWindow(), 500);
  } catch {
    clearVaultPasswords();
    failed.value = true;
  } finally {
    pending.value = false;
  }
}

onMounted(() => {
  window.addEventListener("keydown", onKeydown);
  void load();
});
onBeforeUnmount(() => {
  clearVaultPasswords();
  window.removeEventListener("keydown", onKeydown);
  window.clearTimeout(closeTimer);
});
</script>

<template>
  <NvxSecureWindow
    class="secure-sync"
    :title="t(`plugins.sshSync.prompt.${prompt?.kind ?? 'loading'}.title`)"
    :description="t(`plugins.sshSync.prompt.${prompt?.kind ?? 'loading'}.description`)"
    :layout="asksForPassword ? 'form' : 'review'"
    :danger="isRemoteReset"
  >
    <p
      v-if="loading"
      role="status"
    >
      {{ t("plugins.sshSync.loading") }}
    </p>
    <NvxInlineNotice
      v-else-if="failed"
      tone="error"
      :title="t('plugins.sshSync.failed')"
    />
    <NvxInlineNotice
      v-else-if="completed"
      :title="t(cancelled ? isDataApplyUploaded ? 'plugins.sshSync.dataApplyUploadedCancelled' : isDataApply ? 'plugins.sshSync.dataApplyDownloadedCancelled' : 'plugins.sshSync.cancelled' : 'plugins.sshSync.accepted')"
    />
    <template v-else-if="prompt">
      <dl
        class="secure-sync__requester"
        :class="{ 'secure-sync__requester--comparison': isDirectionChoice || isConflictChoice || isMergedDeletion || isDataApply || isDataReview }"
      >
        <div><dt>{{ t("plugins.sshSync.requestingPlugin") }}</dt><dd>{{ prompt.pluginId }}</dd></div>
        <div><dt>{{ t("plugins.sshSync.providerProfile") }}</dt><dd>{{ prompt.profileId }}</dd></div>
        <div v-if="prompt.remoteOrigin">
          <dt>{{ t("plugins.sshSync.remoteService") }}</dt><dd>{{ prompt.remoteOrigin }}</dd>
        </div>
      </dl>
      <NvxInlineNotice
        v-if="!asksForPassword"
        :tone="isRemoteReset ? 'error' : isDirectionChoice || isConflictChoice || isMergedDeletion || isDataApply || isDataReview ? 'warning' : 'info'"
        :title="t(isRemoteReset ? 'plugins.sshSync.remoteResetWarningTitle' : isDataReview ? 'plugins.sshSync.dataReviewWarningTitle' : isDataApply ? 'plugins.sshSync.dataApplyWarningTitle' : isMergedDeletion ? 'plugins.sshSync.mergedDeletionWarningTitle' : isConflictChoice ? 'plugins.sshSync.conflictWarningTitle' : isDirectionChoice ? 'plugins.sshSync.directionWarningTitle' : 'plugins.sshSync.securityTitle')"
      >
        {{ t(isRemoteReset ? "plugins.sshSync.remoteResetWarningDescription" : isDataReview ? "plugins.sshSync.dataReviewWarningDescription" : isDataApplyUploaded ? "plugins.sshSync.dataApplyUploadedWarningDescription" : isDataApply ? "plugins.sshSync.dataApplyDownloadedWarningDescription" : isMergedDeletion ? "plugins.sshSync.mergedDeletionWarningDescription" : isConflictChoice ? "plugins.sshSync.conflictWarningDescription" : isDirectionChoice ? "plugins.sshSync.directionWarningDescription" : "plugins.sshSync.securityDescription") }}
      </NvxInlineNotice>
      <NvxInlineNotice
        v-if="(isDirectionChoice || isMergedDeletion || isDataApply || isDataReview) && prompt.relatedForwardRuleLabels.length"
        tone="warning"
        :title="t('plugins.sshSync.relatedForwardRulesTitle')"
      >
        <p>{{ t(isDataApply ? "plugins.sshSync.relatedForwardRulesDataApplyHint" : isMergedDeletion ? "plugins.sshSync.relatedForwardRulesMergeHint" : "plugins.sshSync.relatedForwardRulesDirectionHint") }}</p>
        <ul class="secure-sync__forward-rules">
          <li
            v-for="(label, index) in prompt.relatedForwardRuleLabels"
            :key="`${label}-${index}`"
          >
            {{ label }}
          </li>
        </ul>
      </NvxInlineNotice>
      <section
        v-if="prompt.kind === 'authorizeProvider' && prompt.oauth"
        class="secure-sync__oauth"
      >
        <dl class="secure-facts">
          <div><dt>{{ t("plugins.sshSync.authorizationUrl") }}</dt><dd>{{ prompt.oauth.authorizationUrl }}</dd></div>
          <div><dt>{{ t("plugins.sshSync.tokenUrl") }}</dt><dd>{{ prompt.oauth.tokenUrl }}</dd></div>
          <div><dt>{{ t("plugins.sshSync.revokeUrl") }}</dt><dd>{{ prompt.oauth.revokeUrl }}</dd></div>
        </dl>
        <div>
          <strong>{{ t("plugins.sshSync.resourceOrigins") }}</strong>
          <ul>
            <li
              v-for="origin in prompt.oauth.resourceOrigins"
              :key="origin"
            >
              {{ origin }}
            </li>
          </ul>
        </div>
      </section>
      <section
        v-else-if="prompt.kind === 'selectBackup'"
        class="secure-sync__hosts"
      >
        <p>{{ t("plugins.sshSync.desktopScopeHint") }}</p>
        <article
          v-for="profile in prompt.desktopProfiles"
          :key="profile.profileId"
        >
          <NvxCheckbox
            :model-value="selectedDesktops.includes(profile.profileId)"
            :disabled="pending"
            @update:model-value="updateDesktop(profile.profileId, $event)"
          >
            <strong>{{ profile.label }} · {{ profile.protocol.toUpperCase() }}</strong>
            <template #hint>
              {{ profile.username }} · {{ profile.address }}:{{ profile.port }}
            </template>
          </NvxCheckbox>
          <div
            v-if="profile.credentials.length"
            class="secure-sync__credentials"
          >
            <NvxCheckbox
              v-for="credential in profile.credentials"
              :key="credential.credentialRefId"
              :model-value="credentialSelected(credential.credentialRefId)"
              :disabled="pending"
              @update:model-value="updateDesktopCredential(profile.profileId, credential.credentialRefId, $event)"
            >
              {{ credential.label }} · {{ credential.methodLabel }}
            </NvxCheckbox>
          </div>
        </article>
        <article
          v-for="host in prompt.hosts"
          :key="host.hostId"
        >
          <NvxCheckbox
            :model-value="hostSelected(host.hostId)"
            :disabled="pending"
            @update:model-value="updateHost(host.hostId, $event)"
          >
            <strong>{{ host.label }}</strong>
            <template #hint>
              {{ host.endpoint }}
            </template>
          </NvxCheckbox>
          <div
            v-if="host.credentials.length"
            class="secure-sync__credentials"
          >
            <NvxCheckbox
              v-for="credential in host.credentials"
              :key="credential.credentialRefId"
              :model-value="credentialSelected(credential.credentialRefId)"
              :disabled="pending || credential.machineBound"
              @update:model-value="updateCredential(host.hostId, credential.credentialRefId, $event)"
            >
              {{ credential.label }} · {{ credential.methodLabel }}
              <template
                v-if="credential.machineBound"
                #hint
              >
                {{ t("plugins.sshSync.machineBound") }}
              </template>
            </NvxCheckbox>
          </div>
        </article>
      </section>
      <section
        v-else-if="asksForPassword"
        class="secure-sync__password"
      >
        <NvxField
          for-id="sync-vault-password"
          :label="t('plugins.sshSync.vaultPassword')"
          :error="passwordTooShort ? t('plugins.sshSync.passwordTooShort') : existingPasswordTooShort ? t('plugins.sshSync.existingPasswordTooShort') : undefined"
        >
          <div class="secure-sync__password-input">
            <NvxInput
              id="sync-vault-password"
              v-model="vaultPassword"
              autofocus
              :type="passwordVisible ? 'text' : 'password'"
              :autocomplete="createsLocalVault || prompt.kind === 'createRecoveryPassword' ? 'new-password' : 'current-password'"
              :disabled="pending"
              :invalid="passwordTooShort || existingPasswordTooShort"
              aria-describedby="sync-password-hint"
              @keydown.enter.prevent="passwordValid && !pending && decide('approve')"
            />
            <NvxIconButton
              size="sm"
              :label="t(passwordVisible ? 'plugins.sshSync.hidePassword' : 'plugins.sshSync.showPassword')"
              :aria-pressed="passwordVisible"
              :disabled="pending"
              @click="passwordVisible = !passwordVisible"
            >
              <NvxIcon
                :icon="passwordVisible ? EyeOff : Eye"
                :size="20"
              />
            </NvxIconButton>
          </div>
        </NvxField>
        <NvxField
          v-if="createsLocalVault"
          for-id="sync-vault-password-confirmation"
          :label="t('plugins.sshSync.vaultPasswordConfirmation')"
          :hint="t('plugins.sshSync.confirmPasswordHint')"
          :error="confirmationMismatch ? t('plugins.sshSync.passwordMismatch') : undefined"
        >
          <NvxInput
            id="sync-vault-password-confirmation"
            v-model="vaultPasswordConfirmation"
            :type="passwordVisible ? 'text' : 'password'"
            autocomplete="new-password"
            :disabled="pending"
            :invalid="confirmationMismatch"
            @keydown.enter.prevent="passwordValid && !pending && decide('approve')"
          />
        </NvxField>
        <p id="sync-password-hint">
          {{ t(prompt.kind === "createLocalVault" ? "plugins.sshSync.createLocalVaultHint" : prompt.kind === "createRecoveryPassword" ? "plugins.sshSync.createPasswordHint" : prompt.kind === "recoverExistingKey" ? "plugins.sshSync.legacyPasswordHint" : prompt.kind === "recoverSynchronizedKey" ? "plugins.sshSync.recoverSynchronizedKeyHint" : "plugins.sshSync.vaultPasswordHint") }}
        </p>
      </section>
      <section
        v-else-if="isDataReview"
        class="secure-sync__differences"
      >
        <p v-if="!selectedReview">
          {{ t("plugins.sshSync.dataReviewSelectHint") }}
        </p>
        <template v-else>
          <p>{{ t(selectedReview.uploadRequired ? "plugins.sshSync.dataReviewUploadHint" : "plugins.sshSync.dataReviewDownloadHint") }}</p>
          <section
            v-for="side in reviewSides"
            :key="side.name"
            class="secure-sync__review-side"
          >
            <h2>{{ t(`plugins.sshSync.${side.name}`) }}</h2>
            <div class="secure-sync__times">
              <div>
                <strong>{{ t("plugins.sshSync.beforeSync") }}</strong>
                <p>{{ t("plugins.sshSync.sideCounts", { hosts: side.before.hostCount, credentials: side.before.credentialCount, desktops: side.before.desktopProfileCount }) }}</p>
              </div>
              <div>
                <strong>{{ t("plugins.sshSync.afterSync") }}</strong>
                <p>{{ t("plugins.sshSync.sideCounts", { hosts: side.after.hostCount, credentials: side.after.credentialCount, desktops: side.after.desktopProfileCount }) }}</p>
                <span>{{ t("plugins.sshSync.pendingDeletions") }} · {{ side.after.deleteCount }}</span>
              </div>
            </div>
            <div class="secure-sync__difference-heading">
              <strong>{{ t("plugins.sshSync.differenceDetails", { count: side.after.differenceTotalCount }) }}</strong>
            </div>
            <p v-if="!side.after.differenceTotalCount">
              {{ t("plugins.sshSync.dataReviewNoChanges") }}
            </p>
            <div class="secure-sync__difference-list">
              <article
                v-for="(difference, index) in side.after.differences"
                :key="`${difference.kind}-${index}`"
                class="secure-sync__difference-row"
              >
                <div>
                  <strong>{{ difference.label || t(`plugins.sshSync.differenceKinds.${difference.kind}`) }}</strong>
                  <span>{{ t(`plugins.sshSync.differenceKinds.${difference.kind}`) }}</span>
                </div>
                <span class="secure-sync__change">{{ t(`plugins.sshSync.applyDifferenceChanges.${difference.change}`) }}</span>
                <div class="secure-sync__side-value">
                  <span>{{ t("plugins.sshSync.beforeSync") }}</span>
                  <strong>{{ differenceSummary(difference, "local") }}</strong>
                </div>
                <div class="secure-sync__side-value">
                  <span>{{ t("plugins.sshSync.afterSync") }}</span>
                  <strong>{{ differenceSummary(difference, "remote") }}</strong>
                </div>
              </article>
            </div>
            <p v-if="side.after.differenceOmittedCount">
              {{ t("plugins.sshSync.moreDifferences", { count: side.after.differenceOmittedCount }) }}
            </p>
            <aside
              v-if="side.after.relatedForwardRuleLabels.length"
              class="secure-sync__related-rules"
            >
              <strong>{{ t("plugins.sshSync.relatedForwardRulesTitle") }}</strong>
              <p>{{ t("plugins.sshSync.relatedForwardRulesDataApplyHint") }}</p>
              <ul>
                <li
                  v-for="label in side.after.relatedForwardRuleLabels"
                  :key="label"
                >
                  {{ label }}
                </li>
              </ul>
            </aside>
          </section>
        </template>
      </section>
      <section
        v-else-if="prompt.kind === 'chooseSyncDirection' || prompt.kind === 'resolveConflicts' || prompt.kind === 'approveMergedDeletion' || prompt.kind === 'approveDataApply'"
        class="secure-sync__differences"
      >
        <div class="secure-sync__times">
          <div>
            <strong>{{ t(isDataApply ? "plugins.sshSync.beforeApply" : isMergedDeletion ? "plugins.sshSync.beforeMerge" : "plugins.sshSync.local") }}</strong>
            <p>{{ t("plugins.sshSync.sideCounts", { hosts: prompt.hostCount, credentials: prompt.credentialCount, desktops: prompt.desktopProfileCount }) }}</p>
            <span>{{ t("plugins.sshSync.localComparedAt") }}</span>
            <strong>{{ formatTime(prompt.localComparedAtUnixMs) }}</strong>
          </div>
          <div>
            <strong>{{ t(isDataApply ? "plugins.sshSync.afterApply" : isMergedDeletion ? "plugins.sshSync.afterMerge" : "plugins.sshSync.remote") }}</strong>
            <p>{{ t("plugins.sshSync.sideCounts", { hosts: prompt.remoteHostCount, credentials: prompt.remoteCredentialCount, desktops: prompt.remoteDesktopProfileCount }) }}</p>
            <span>{{ t(isDataApply || isMergedDeletion ? "plugins.sshSync.pendingDeletions" : "plugins.sshSync.remoteUpdatedAt") }}</span>
            <strong>{{ isDataApply || isMergedDeletion ? prompt.deleteCount : formatTime(prompt.remoteUpdatedAtUnixMs) }}</strong>
          </div>
        </div>
        <p v-if="!isMergedDeletion && !isDataApply">
          {{ t("plugins.sshSync.comparisonTimeHint") }}
        </p>
        <div class="secure-sync__difference-heading">
          <strong>{{ t("plugins.sshSync.differenceDetails", { count: prompt.differenceTotalCount }) }}</strong>
        </div>
        <div class="secure-sync__difference-list">
          <article
            v-for="(difference, index) in prompt.differences"
            :key="`${difference.kind}-${difference.label}-${index}`"
            class="secure-sync__difference-row"
          >
            <div>
              <strong>{{ difference.label || t(`plugins.sshSync.differenceKinds.${difference.kind}`) }}</strong>
              <span>{{ t(`plugins.sshSync.differenceKinds.${difference.kind}`) }}</span>
            </div>
            <span class="secure-sync__change">{{ t(`plugins.sshSync.${isDataApply ? 'applyDifferenceChanges' : 'differenceChanges'}.${difference.change}`) }}</span>
            <div class="secure-sync__side-value">
              <span>{{ t(isDataApply ? "plugins.sshSync.beforeApply" : isMergedDeletion ? "plugins.sshSync.beforeMerge" : "plugins.sshSync.local") }}</span>
              <strong>{{ differenceSummary(difference, "local") }}</strong>
            </div>
            <div class="secure-sync__side-value">
              <span>{{ t(isDataApply ? "plugins.sshSync.afterApply" : isMergedDeletion ? "plugins.sshSync.afterMerge" : "plugins.sshSync.remote") }}</span>
              <strong>{{ differenceSummary(difference, "remote") }}</strong>
            </div>
          </article>
        </div>
        <p v-if="prompt.differenceOmittedCount">
          {{ t("plugins.sshSync.moreDifferences", { count: prompt.differenceOmittedCount }) }}
        </p>
      </section>
      <dl
        v-else
        class="secure-facts"
      >
        <div>
          <dt>{{ t("plugins.sshSync.hosts") }}</dt><dd>{{ prompt.hostCount }}</dd>
        </div>
        <div>
          <dt>{{ t("plugins.sshSync.credentials") }}</dt><dd>{{ prompt.credentialCount }}</dd>
        </div>
        <div>
          <dt>{{ t("plugins.sshSync.conflicts") }}</dt><dd>{{ prompt.conflictCount }}</dd>
        </div>
        <div v-if="prompt.updateCount">
          <dt>{{ t("plugins.sshSync.updates") }}</dt><dd>{{ prompt.updateCount }}</dd>
        </div>
        <div v-if="prompt.deleteCount">
          <dt>{{ t("plugins.sshSync.deletions") }}</dt><dd>{{ prompt.deleteCount }}</dd>
        </div>
      </dl>
    </template>
    <template
      v-if="prompt && requiresDirectionChoice && !loading && !failed && !completed"
      #decision
    >
      <fieldset
        class="secure-sync__direction"
        :disabled="pending"
      >
        <legend>
          {{ t((isConflictChoice || isDataReview) ? "plugins.sshSync.conflictChoiceLabel" : "plugins.sshSync.directionChoiceLabel") }}
        </legend>
        <div class="secure-sync__direction-options">
          <label>
            <input
              v-model="selectedDirection"
              type="radio"
              name="sync-direction"
              value="keepLocal"
            >
            <span>
              <strong>{{ t((isConflictChoice || isDataReview) ? "plugins.sshSync.keepLocal" : "plugins.sshSync.localOverRemote") }}</strong>
              <small>{{ t(isConflictChoice || isDataReview ? "plugins.sshSync.keepLocalConflictHint" : "plugins.sshSync.keepLocalDirectionHint") }}</small>
            </span>
          </label>
          <label>
            <input
              v-model="selectedDirection"
              type="radio"
              name="sync-direction"
              value="useRemote"
            >
            <span>
              <strong>{{ t((isConflictChoice || isDataReview) ? "plugins.sshSync.useRemote" : "plugins.sshSync.remoteOverLocal") }}</strong>
              <small>{{ t(isConflictChoice || isDataReview ? "plugins.sshSync.useRemoteConflictHint" : "plugins.sshSync.useRemoteDirectionHint") }}</small>
            </span>
          </label>
        </div>
      </fieldset>
    </template>
    <template #actions>
      <NvxButton
        variant="secondary"
        :disabled="pending"
        @click="cancel"
      >
        {{ t(failed || completed ? "plugins.sshSync.close" : "plugins.sshSync.cancel") }}
      </NvxButton>
      <template v-if="prompt && !loading && !failed && !completed">
        <NvxButton
          :variant="isRemoteReset || isDirectionChoice || isMergedDeletion || isDataApply || isDataReview ? 'danger' : 'primary'"
          :loading="pending"
          :disabled="!passwordValid || (requiresDirectionChoice && !selectedDirection) || (isDataReview && !selectedReview)"
          @click="decide(isMergedDeletion ? 'applyMerged' : requiresDirectionChoice && selectedDirection ? selectedDirection : 'approve')"
        >
          {{ t(isDataReview ? "plugins.sshSync.confirmSync" : isDataApply ? "plugins.sshSync.approveDataApply" : isMergedDeletion ? "plugins.sshSync.approveMergedDeletion" : "window.approveOnce") }}
        </NvxButton>
      </template>
    </template>
    <template
      v-if="asksForPassword && !failed && !completed"
      #support
    >
      <div class="secure-sync__assurance">
        <div>
          <strong>{{ t("plugins.sshSync.securityTitle") }}</strong>
          <p>{{ t("plugins.sshSync.passwordLocal") }}</p>
          <details>
            <summary>{{ t("plugins.sshSync.securityDetails") }}</summary>
            <p>{{ t("plugins.sshSync.securityDescription") }}</p>
          </details>
        </div>
      </div>
    </template>
  </NvxSecureWindow>
</template>

<style scoped>
.secure-sync__requester { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); gap: var(--nvx-space-2); margin: 0; }
.secure-sync__requester > div { min-width: 0; }
.secure-sync__requester > div + div { border-inline-start: var(--nvx-border-width) solid var(--nvx-color-border); padding-inline-start: var(--nvx-space-3); }
.secure-sync__requester dt { font-size: var(--nvx-font-size-sm); }
.secure-sync__requester dd { margin-top: var(--nvx-space-1); font-weight: var(--nvx-font-weight-semibold); }
.secure-sync__password { display: grid; gap: var(--nvx-space-2); }
.secure-sync__password > p { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.secure-sync__password-input { position: relative; }
.secure-sync__password-input :deep(.nvx-input) { padding-inline-end: 42px; }
.secure-sync__password-input :deep(.nvx-icon-button) { position: absolute; inset-inline-end: var(--nvx-space-1); top: 4px; }
.secure-sync__assurance strong { color: var(--nvx-color-text-primary); }
.secure-sync__assurance p { margin: var(--nvx-space-1) 0 0; }
.secure-sync__assurance details { margin-top: var(--nvx-space-1); }
.secure-sync__assurance summary { cursor: pointer; color: var(--nvx-color-text-secondary); }
.secure-sync__oauth { display: grid; gap: var(--nvx-space-2); overflow-wrap: anywhere; }
.secure-sync__oauth ul { margin: var(--nvx-space-1) 0 0; padding-inline-start: var(--nvx-space-4); }
.secure-sync__hosts { display: grid; }
.secure-sync__hosts article { display: grid; gap: var(--nvx-space-2); padding-block: var(--nvx-space-2); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.secure-sync__credentials { display: grid; gap: var(--nvx-space-2); padding-inline-start: var(--nvx-space-4); }
.secure-sync__differences { min-width: 0; }
.secure-sync__forward-rules { margin: var(--nvx-space-1) 0 0; padding-inline-start: var(--nvx-space-4); overflow-wrap: anywhere; }
.secure-sync__review-side { display: grid; gap: var(--nvx-space-2); }
.secure-sync__review-side h2 { font-size: var(--nvx-font-size-md); margin: 0; }
.secure-sync__times { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--nvx-space-2); }
.secure-sync__times > div { display: grid; gap: var(--nvx-space-1); padding: var(--nvx-space-2); border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); }
.secure-sync__times span, .secure-sync__difference-heading span, .secure-sync__difference-row > div > span, .secure-sync__side-value > span, .secure-sync__differences > p { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.secure-sync__times strong { overflow-wrap: anywhere; }
.secure-sync__times p { margin-bottom: var(--nvx-space-1); }
.secure-sync__difference-heading { display: flex; flex-wrap: wrap; align-items: baseline; justify-content: space-between; gap: var(--nvx-space-2); }
.secure-sync__difference-list { display: grid; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); overflow: hidden; }
.secure-sync__difference-row { display: grid; grid-template-columns: minmax(100px, 1.25fr) auto minmax(88px, 1fr) minmax(88px, 1fr); gap: var(--nvx-space-2); align-items: center; min-width: 0; padding: var(--nvx-space-2); background: var(--nvx-color-bg-surface); }
.secure-sync__difference-row + .secure-sync__difference-row { border-top: var(--nvx-border-width) solid var(--nvx-color-border); }
.secure-sync__difference-row > div, .secure-sync__side-value { display: grid; gap: var(--nvx-space-1); min-width: 0; }
.secure-sync__difference-row strong { min-width: 0; overflow-wrap: anywhere; }
.secure-sync__change { padding: var(--nvx-space-1) var(--nvx-space-2); border-radius: var(--nvx-radius-sm); background: var(--nvx-color-warning-soft); color: var(--nvx-color-warning); font-size: var(--nvx-font-size-xs); font-weight: var(--nvx-font-weight-semibold); white-space: nowrap; }
.secure-sync__direction { display: grid; gap: var(--nvx-space-2); min-width: 0; margin: 0; padding: 0; border: 0; }
.secure-sync__direction legend { padding-inline: var(--nvx-space-1); font-size: var(--nvx-font-size-sm); font-weight: var(--nvx-font-weight-semibold); }
.secure-sync__direction-options { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--nvx-space-2); }
.secure-sync__direction label { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: var(--nvx-space-2); align-items: start; min-width: 0; padding: var(--nvx-space-2); border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-sm); cursor: pointer; }
.secure-sync__direction input { margin: 2px 0 0; accent-color: var(--nvx-color-accent); }
.secure-sync__direction label > span { display: grid; gap: var(--nvx-space-1); min-width: 0; }
.secure-sync__direction small { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.secure-sync__direction label:has(input:checked) { border-color: var(--nvx-color-accent); background: var(--nvx-color-accent-soft); }
.secure-sync__direction:disabled label { cursor: default; opacity: .7; }

@media (max-width: 520px) {
  .secure-sync__requester { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--nvx-space-3); }
  .secure-sync__requester > div + div { border: 0; padding: 0; }
  .secure-sync__requester--comparison > div { display: grid; grid-template-columns: minmax(90px, .4fr) minmax(0, 1fr); gap: var(--nvx-space-3); align-items: baseline; }
  .secure-sync__requester--comparison dd { margin: 0; }
  .secure-sync__times { grid-template-columns: minmax(0, 1fr); }
  .secure-sync__direction-options { grid-template-columns: minmax(0, 1fr); }
}
</style>
