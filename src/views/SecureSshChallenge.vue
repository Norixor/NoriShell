<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";
import { NvxButton, NvxField, NvxInlineNotice, NvxInput } from "../components/ui";
import { secureSshChallengeClient, type SecureChallengePrompt } from "../core-api/secure-ssh-challenge-client";
const { t } = useI18n();
const id = new URLSearchParams(window.location.search).get("prompt") ?? "";
const prompt = ref<SecureChallengePrompt | null>(null);
const answers = ref<string[]>([]);
const pending = ref(false);
const failed = ref(false);
const content = computed(() => prompt.value?.content);
const keyboard = computed(() => content.value?.kind === "sshKeyboard" || content.value?.kind === "metricsKeyboard");
const fields = computed(() => {
  const value = content.value;
  if (value?.kind === "sshKeyboard") return value.challenge.prompts.map((p) => ({ index: p.promptIndex, label: p.text, secret: p.sensitive || !p.echo }));
  if (value?.kind === "metricsKeyboard") return value.challenge.prompts.map((p) => ({ index: p.promptIndex, label: p.label, secret: !p.echo }));
  return [];
});
const key = computed(() => {
  const value = content.value;
  if (value?.kind === "sshHostKey") return { endpoint: `${value.challenge.endpoint.address}:${value.challenge.endpoint.port}`, algorithm: value.challenge.keyAlgorithm, fingerprint: value.challenge.fingerprintSha256, mismatch: false };
  if (value?.kind === "sftpHostKey") return { endpoint: value.challenge.endpoint, algorithm: value.challenge.algorithm, fingerprint: value.challenge.fingerprintSha256, mismatch: false };
  if (value?.kind === "metricsHostKey") return { endpoint: value.challenge.hostId, algorithm: value.challenge.algorithm, fingerprint: value.challenge.fingerprintSha256, mismatch: value.challenge.trustedFingerprintSha256 !== null };
  return null;
});
const instructions = computed(() => {
  const value = content.value;
  if (value?.kind === "sshKeyboard") return { name: value.challenge.name, text: value.challenge.instructions };
  if (value?.kind === "metricsKeyboard") return { name: value.challenge.name, text: value.challenge.instruction };
  return null;
});
function clear() { answers.value = fields.value.map(() => ""); }
async function decide(approved: boolean) {
  if (!prompt.value || pending.value || (approved && key.value?.mismatch)) return;
  pending.value = true;
  const request = secureSshChallengeClient.submit(id, approved, approved ? [...answers.value] : []);
  clear();
  try { await request; } catch { failed.value = true; pending.value = false; }
}
onMounted(async () => { try { prompt.value = await secureSshChallengeClient.get(id); clear(); } catch { failed.value = true; } });
onBeforeUnmount(clear);
</script>
<template>
  <NvxSecureWindow
    :title="t(keyboard ? 'sshSession.keyboardInteractive.title' : 'sshSession.hostKey.title')"
    :description="t(keyboard ? 'sshSession.keyboardInteractive.description' : 'sshSession.hostKey.description')"
    layout="form"
  >
    <dl
      v-if="key"
      class="secure-challenge-details"
    >
      <dt>{{ t('sshSession.hostKey.endpoint') }}</dt><dd>{{ key.endpoint }}</dd>
      <dt>{{ t('sshSession.hostKey.algorithm') }}</dt><dd>{{ key.algorithm }}</dd>
      <dt>{{ t('sshSession.hostKey.fingerprint') }}</dt><dd><code>{{ key.fingerprint }}</code></dd>
    </dl>
    <form
      v-if="keyboard"
      class="secure-challenge-fields"
      @submit.prevent="decide(true)"
    >
      <strong v-if="instructions?.name">{{ instructions.name }}</strong>
      <p v-if="instructions?.text">
        {{ instructions.text }}
      </p>
      <NvxField
        v-for="(field, index) in fields"
        :key="field.index"
        :for-id="`secure-answer-${field.index}`"
        :label="field.label || t('sshSession.keyboardInteractive.answer')"
      >
        <NvxInput
          :id="`secure-answer-${field.index}`"
          :model-value="answers[index] ?? ''"
          :type="field.secret ? 'password' : 'text'"
          :disabled="pending"
          :maxlength="65536"
          autocomplete="off"
          @update:model-value="answers[index] = $event"
        />
      </NvxField>
      <button
        type="submit"
        hidden
        tabindex="-1"
      />
    </form>
    <NvxInlineNotice
      v-if="failed"
      tone="error"
      :title="t('sshSession.keyboardInteractive.failed')"
    />
    <template #actions>
      <NvxButton
        variant="secondary"
        :disabled="pending || !prompt"
        @click="decide(false)"
      >
        {{ t(keyboard ? 'sshSession.keyboardInteractive.cancel' : 'sshSession.hostKey.reject') }}
      </NvxButton>
      <NvxButton
        :loading="pending"
        :disabled="pending || !prompt || key?.mismatch"
        @click="decide(true)"
      >
        {{ t(keyboard ? 'sshSession.keyboardInteractive.continue' : 'sshSession.hostKey.accept') }}
      </NvxButton>
    </template>
  </NvxSecureWindow>
</template>
<style scoped>
.secure-challenge-fields { display: grid; gap: var(--nvx-space-4); }
.secure-challenge-details dd { margin: 0 0 var(--nvx-space-3); overflow-wrap: anywhere; }
.secure-challenge-details dt { color: var(--nvx-color-text-muted); }
</style>
