<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import { NvxButton, NvxCheckbox, NvxDialog, NvxField, NvxInlineNotice, NvxInput, NvxSelect } from "../ui";
import NvxDesktopDisplaySettings from "./NvxDesktopDisplaySettings.vue";
import NvxDesktopPasswordFields from "./NvxDesktopPasswordFields.vue";
import { desktopClient } from "../../core-api/desktop-client";
import { listCredentialRefs, listHosts, listIdentities, parseCoreApiError } from "../../core-api/client";
import type { CredentialRefSummary, DesktopProfile, HostSummary, VncProtocolVersion } from "../../core-api/generated/core-api";

const props = defineProps<{ profileId?: string }>();
const emit = defineEmits<{ saved: [profile: DesktopProfile]; cancel: []; closeCancelled: [] }>();
const { t, te } = useI18n();

const hosts = ref<HostSummary[]>([]);
const credentials = ref<CredentialRefSummary[]>([]);
const draft = ref<DesktopProfile>(fresh());
const baseline = ref(signature(draft.value));
const passwordFields = ref<InstanceType<typeof NvxDesktopPasswordFields> | null>(null);
const passwordDirty = ref(false);
const loading = ref(true), failed = ref(false), invalid = ref(false), busy = ref(false), closing = ref(false), confirmClose = ref(false);
const failureText = ref("");
const failureCode = ref("");
function showFailure(error: unknown) {
  const failure = parseCoreApiError(error);
  failureText.value = (failure?.messageKey && te(failure.messageKey) ? t(failure.messageKey, failure.params) : t("desktop.error"))
    + (failure?.diagnosticId ? ` ${t("diagnostics.id", { id: failure.diagnosticId })}` : "");
  failureCode.value = failure?.code ?? "";
  failed.value = true;
}
const gateways = computed(() => [{ value: "", label: t("desktop.direct") }, ...hosts.value.map((host) => ({ value: host.hostId, label: host.label }))]);
const dirty = computed(() => signature(draft.value) !== baseline.value || passwordDirty.value);

let disposed = false;
let loadEpoch = 0;
let finishClose: ((value: boolean) => void) | undefined;

function fresh(): DesktopProfile {
  return {
    id: crypto.randomUUID(), label: "", protocol: "rdp", address: "", port: 3389,
    username: "", domain: "", hostId: null, gatewayHostId: null, credentialRefId: null,
    width: 1280, height: 800, rdpTransportMode: "auto", rdpGraphicsMode: "auto", rdpResolutionMode: "fixed", vncResolutionMode: "server", clipboardEnabled: false, audioPlaybackEnabled: false, vncProtocolVersion: "auto", revision: "0",
  };
}

function signature(profile: DesktopProfile) {
  return JSON.stringify(profile);
}

async function load() {
  const epoch = ++loadEpoch;
  loading.value = true;
  failed.value = false;
  failureText.value = "";
  failureCode.value = "";
  invalid.value = false;
  try {
    const [savedProfiles, savedHosts, identities] = await Promise.all([
      desktopClient.profiles(), listHosts(), listIdentities(),
    ]);
    const savedCredentials = (await Promise.all(
      identities.map((identity) => listCredentialRefs(identity.identityId)),
    )).flat();
    if (disposed || epoch !== loadEpoch) return;
    const profile = props.profileId
      ? savedProfiles.find((item) => item.id === props.profileId)
      : undefined;
    if (props.profileId && !profile) {
      failureText.value = t("desktop.profileErrors.notFound");
      failed.value = true;
      return;
    }
    draft.value = profile ? { ...profile } : fresh();
    baseline.value = signature(draft.value);
    passwordDirty.value = false;
    hosts.value = savedHosts;
    credentials.value = savedCredentials;
  } catch (error) {
    if (!disposed && epoch === loadEpoch) showFailure(error);
  } finally {
    if (!disposed && epoch === loadEpoch) loading.value = false;
  }
}

function changeProtocol(protocol: string) {
  if (protocol !== "rdp" && protocol !== "vnc") return;
  draft.value.protocol = protocol;
  if (protocol === "vnc") {
    draft.value.audioPlaybackEnabled = false;
    draft.value.rdpTransportMode = "auto";
    draft.value.rdpGraphicsMode = "auto";
    draft.value.rdpResolutionMode = "fixed";
    draft.value.username = "";
    draft.value.domain = "";
  } else {
    draft.value.vncProtocolVersion = "auto";
    draft.value.vncResolutionMode = "server";
  }
  draft.value.port = protocol === "rdp" ? 3389 : 5900;
}

function changeVncProtocolVersion(version: string) {
  if (["auto", "rfb33", "rfb37", "rfb38"].includes(version)) {
    draft.value.vncProtocolVersion = version as VncProtocolVersion;
  }
}

function valid(profile: DesktopProfile) {
  return !!profile.label.trim()
    && !!profile.address.trim()
    && Number.isInteger(profile.port) && profile.port >= 1 && profile.port <= 65535
    && profile.width * profile.height <= 16_777_216
    && [profile.width, profile.height].every((value) => Number.isInteger(value) && value >= 200 && value <= 8192);
}

async function save() {
  if (busy.value || loading.value) return;
  const value = { ...draft.value };
  if (value.protocol === "vnc") {
    value.username = "";
    value.domain = "";
  }
  if (!valid(value)) {
    invalid.value = true;
    return;
  }
  invalid.value = false;
  failed.value = false;
  busy.value = true;
  try {
    const fields = passwordFields.value;
    const passwordStage = await fields?.prepare() ?? null;
    if (passwordStage === false || disposed || draft.value.id !== value.id) return;
    const saved = await desktopClient.save({ ...value, label: value.label.trim(), address: value.address.trim() }, passwordStage);
    fields?.acceptSaved();
    if (disposed) return;
    draft.value = { ...saved };
    baseline.value = signature(saved);
    emit("saved", saved);
    if (passwordStage) {
      // Saving succeeded even when the subsequent option refresh is temporarily unavailable.
      try {
        const identities = await listIdentities();
        const updated = (await Promise.all(identities.map((identity) => listCredentialRefs(identity.identityId)))).flat();
        if (!disposed) credentials.value = updated;
      } catch { /* The saved profile remains authoritative until the next editor load. */ }
    }
  } catch (error) {
    if (!disposed) showFailure(error);
  } finally {
    busy.value = false;
  }
}

function resolveClose(value: boolean) {
  confirmClose.value = false;
  const resolve = finishClose;
  finishClose = undefined;
  if (!value && resolve) emit("closeCancelled");
  resolve?.(value);
}

async function discardPasswordFields(): Promise<boolean> {
  const fields = passwordFields.value;
  return !fields || await fields.discard();
}

async function discardAndClose() {
  if (closing.value) return;
  closing.value = true;
  try { resolveClose(await discardPasswordFields()); }
  finally { closing.value = false; }
}

/**
 * The native profile window awaits this method before closing. A dirty draft stays open
 * until the user explicitly confirms discarding it, so window-close and in-page cancel
 * share one close path.
 */
function requestClose(): Promise<boolean> {
  if (busy.value || loading.value || closing.value || finishClose) return Promise.resolve(false);
  if (!dirty.value) return discardPasswordFields();
  confirmClose.value = true;
  return new Promise<boolean>((resolve) => { finishClose = resolve; });
}

watch(() => props.profileId, () => { void load(); }, { immediate: true });
onBeforeUnmount(() => {
  disposed = true;
  finishClose?.(false);
  finishClose = undefined;
});

const closeBusy = computed(() => loading.value || busy.value || closing.value);
defineExpose({ requestClose, dirty, busy: closeBusy });
</script>

<template>
  <section
    class="desktop-profile-editor"
    :aria-busy="loading || busy"
  >
    <NvxInlineNotice
      v-if="loading"
      :title="t('desktop.loading')"
    />
    <NvxInlineNotice
      v-else-if="failed"
      tone="error"
      :title="failureText || t('desktop.error')"
    >
      <span v-if="failureCode">{{ failureCode }}</span>
      <NvxButton
        variant="ghost"
        @click="load"
      >
        {{ t('desktop.refresh') }}
      </NvxButton>
    </NvxInlineNotice>
    <form
      v-else
      class="desktop-profile-editor__form"
      :inert="busy"
      @submit.prevent="save"
    >
      <NvxInlineNotice
        v-if="invalid"
        class="desktop-profile-editor__wide"
        tone="error"
        :title="t('desktop.invalid')"
      />
      <NvxField
        for-id="desktop-label"
        :label="t('desktop.label')"
      >
        <NvxInput
          id="desktop-label"
          v-model="draft.label"
          :maxlength="128"
        />
      </NvxField>
      <NvxField :label="t('desktop.protocol')">
        <NvxSelect
          :model-value="draft.protocol"
          :aria-label="t('desktop.protocol')"
          :options="[{ value: 'rdp', label: 'RDP' }, { value: 'vnc', label: 'VNC' }]"
          @update:model-value="changeProtocol"
        />
      </NvxField>
      <NvxField
        v-if="draft.protocol === 'vnc'"
        :label="t('desktop.vncProtocolVersion')"
      >
        <NvxSelect
          :model-value="draft.vncProtocolVersion"
          :aria-label="t('desktop.vncProtocolVersion')"
          :options="[
            { value: 'auto', label: t('desktop.vncVersions.auto') },
            { value: 'rfb33', label: 'RFB 3.3' },
            { value: 'rfb37', label: 'RFB 3.7' },
            { value: 'rfb38', label: 'RFB 3.8' },
          ]"
          @update:model-value="changeVncProtocolVersion"
        />
      </NvxField>
      <NvxField
        for-id="desktop-address"
        :label="t('desktop.address')"
      >
        <NvxInput
          id="desktop-address"
          v-model="draft.address"
          :maxlength="253"
        />
      </NvxField>
      <p
        v-if="draft.protocol === 'vnc'"
        class="desktop-profile-editor__wide desktop-profile-editor__hint"
      >
        {{ t('desktop.vncProtocolVersionHint') }}
      </p>
      <NvxField
        for-id="desktop-port"
        :label="t('desktop.port')"
      >
        <NvxInput
          id="desktop-port"
          :model-value="String(draft.port)"
          type="number"
          :min="1"
          :max="65535"
          @update:model-value="draft.port = Number($event)"
        />
      </NvxField>
      <NvxField
        v-if="draft.protocol === 'rdp'"
        for-id="desktop-user"
        :label="t('desktop.username')"
      >
        <NvxInput
          id="desktop-user"
          v-model="draft.username"
        />
      </NvxField>
      <NvxField
        v-if="draft.protocol === 'rdp'"
        for-id="desktop-domain"
        :label="t('desktop.domain')"
      >
        <NvxInput
          id="desktop-domain"
          v-model="draft.domain"
        />
      </NvxField>
      <NvxDesktopPasswordFields
        :key="draft.id"
        ref="passwordFields"
        v-model="draft.credentialRefId"
        :new-profile="draft.revision === '0'"
        :credentials="credentials"
        :label="draft.label.trim() || draft.address.trim()"
        :busy="busy"
        @dirty-change="passwordDirty = $event"
      />
      <NvxField
        class="desktop-profile-editor__wide"
        :label="t('desktop.gateway')"
      >
        <NvxSelect
          :model-value="draft.gatewayHostId ?? ''"
          :options="gateways"
          :aria-label="t('desktop.gateway')"
          @update:model-value="draft.gatewayHostId = $event || null"
        />
      </NvxField>
      <NvxDesktopDisplaySettings
        v-model="draft"
        class="desktop-profile-editor__wide"
      />
      <NvxCheckbox
        v-if="draft.protocol === 'rdp'"
        v-model="draft.audioPlaybackEnabled"
        class="desktop-profile-editor__wide"
      >
        {{ t('desktop.audioPlayback') }}<template #hint>
          {{ t('desktop.audioPlaybackHint') }}
        </template>
      </NvxCheckbox>
      <NvxCheckbox
        v-model="draft.clipboardEnabled"
        class="desktop-profile-editor__wide"
      >
        {{ t('desktop.clipboard') }}<template #hint>
          {{ t('desktop.clipboardHint') }}
        </template>
      </NvxCheckbox>
      <footer class="desktop-profile-editor__actions desktop-profile-editor__wide">
        <NvxButton
          variant="secondary"
          :disabled="busy"
          @click="emit('cancel')"
        >
          {{ t('desktop.cancel') }}
        </NvxButton>
        <NvxButton
          type="submit"
          :loading="busy"
        >
          {{ t('desktop.save') }}
        </NvxButton>
      </footer>
    </form>
    <NvxDialog
      :model-value="confirmClose"
      plugin-protected
      :title="t('toolWindows.unsavedTitle')"
      :description="t('toolWindows.unsavedDescription')"
      :close-label="t('toolWindows.keepEditing')"
      :dismissible="!busy && !closing"
      @update:model-value="!$event && resolveClose(false)"
    >
      <template #actions>
        <NvxButton
          variant="secondary"
          :disabled="busy || closing"
          @click="resolveClose(false)"
        >
          {{ t('toolWindows.keepEditing') }}
        </NvxButton>
        <NvxButton
          variant="danger"
          :loading="closing"
          :disabled="busy"
          @click="discardAndClose"
        >
          {{ t('toolWindows.discard') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.desktop-profile-editor { height: 100%; min-height: 0; overflow: auto; padding: var(--nvx-space-4); }
.desktop-profile-editor__form { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--nvx-space-3); }
.desktop-profile-editor__wide { grid-column: 1 / -1; }
.desktop-profile-editor__hint { margin: calc(-1 * var(--nvx-space-2)) 0 0; color: var(--nvx-color-text-tertiary); font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-xs); }
.desktop-profile-editor__actions { display: flex; justify-content: flex-end; gap: var(--nvx-space-2); padding-top: var(--nvx-space-2); }
@media (max-width: 560px) { .desktop-profile-editor__form { grid-template-columns: 1fr; } }
</style>
