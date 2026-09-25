<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import NvxSecureWindow from "../components/layout/NvxSecureWindow.vue";
import { NvxButton, NvxField, NvxInlineNotice, NvxInput } from "../components/ui";
import { desktopClient } from "../core-api/desktop-client";
import type { DesktopPrompt } from "../core-api/generated/core-api";
import { MIN_NEW_SECRET_PASSWORD_CHARACTERS, passwordCharacterCount } from "../password-policy";
const { t } = useI18n();
const prompt = ref<DesktopPrompt | null>(null);
const loading = ref(true);
const pending = ref(false);
const failed = ref(false);
const done = ref(false);

const username = ref("");
const domain = ref("");
const password = ref("");
const passwordConfirmation = ref("");
const answers = ref<string[]>([]);

const id = new URLSearchParams(window.location.search).get("prompt") ?? "";
const isVaultCreate = computed(() => prompt.value?.prompt.kind === "vaultCreate");
const passwordTooShort = computed(() => isVaultCreate.value && !!password.value
  && passwordCharacterCount(password.value) < MIN_NEW_SECRET_PASSWORD_CHARACTERS);
const confirmationMismatch = computed(() => isVaultCreate.value && !!passwordConfirmation.value
  && password.value !== passwordConfirmation.value);
const canApprove = computed(() => (
  !isVaultCreate.value
  || (
    passwordCharacterCount(password.value) >= MIN_NEW_SECRET_PASSWORD_CHARACTERS
    && password.value === passwordConfirmation.value
  )
));

function clear() {
  password.value = "";
  passwordConfirmation.value = "";
  answers.value = [];
}

async function decide(approved: boolean) {
  if (!prompt.value || pending.value || (approved && !canApprove.value)) return;
  pending.value = true;
  failed.value = false;

  // Secrets live only in this isolated renderer and the immediately submitted request.
  const request = desktopClient.decide({
    id,
    approved,
    username: approved && prompt.value.prompt.kind === "credentials" && !prompt.value.prompt.passwordOnly ? username.value : null,
    domain: approved && prompt.value.prompt.kind === "credentials" && !prompt.value.prompt.passwordOnly ? domain.value : null,
    password: approved && ["credentials", "vaultCreate", "vaultUnlock"].includes(prompt.value.prompt.kind)
      ? password.value
      : null,
    passwordConfirmation: approved && prompt.value.prompt.kind === "vaultCreate"
      ? passwordConfirmation.value
      : undefined,
    answers: approved ? [...answers.value] : [],
  });
  clear();
  try {
    await request;
    done.value = true;
    await getCurrentWindow().close();
  } catch {
    failed.value = true;
  } finally {
    pending.value = false;
  }
}

onMounted(async () => {
  try {
    prompt.value = await desktopClient.prompt(id);
    const kind = prompt.value.prompt;
    if (kind.kind === "credentials") {
      username.value = kind.username;
      domain.value = kind.domain;
    }
    if (kind.kind === "keyboardInteractive") {
      answers.value = kind.prompts.map(() => "");
    }
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
});

onBeforeUnmount(clear);
</script>
<template>
  <NvxSecureWindow
    :title="t('desktop.secureTitle')"
    :description="t('desktop.secureDescription')"
    layout="form"
  >
    <NvxInlineNotice
      v-if="loading"
      :title="t('desktop.loading')"
    />
    <NvxInlineNotice
      v-if="failed"
      tone="error"
      :title="t('desktop.error')"
    />
    <NvxInlineNotice
      v-if="done"
      :title="t('desktop.done')"
    />
    <div
      v-if="prompt && !done"
      class="desktop-secure"
    >
      <h2>{{ prompt.label }}</h2>
      <template v-if="prompt.prompt.kind === 'credentials'">
        <p>{{ t('desktop.credentials') }}</p>
        <NvxField
          v-if="!prompt.prompt.passwordOnly"
          for-id="desktop-secure-user"
          :label="t('desktop.username')"
        >
          <NvxInput
            id="desktop-secure-user"
            v-model="username"
            :disabled="pending"
            :maxlength="256"
          />
        </NvxField>
        <NvxField
          v-if="!prompt.prompt.passwordOnly"
          for-id="desktop-secure-domain"
          :label="t('desktop.domain')"
        >
          <NvxInput
            id="desktop-secure-domain"
            v-model="domain"
            :disabled="pending"
            :maxlength="256"
          />
        </NvxField>
        <NvxField
          for-id="desktop-secure-password"
          :label="t('desktop.password')"
        >
          <NvxInput
            id="desktop-secure-password"
            v-model="password"
            type="password"
            autocomplete="off"
            :disabled="pending"
            :maxlength="4096"
            @keydown.enter="decide(true)"
          />
        </NvxField>
      </template>
      <template v-else-if="prompt.prompt.kind === 'vaultUnlock'">
        <p>{{ t('desktop.vaultUnlock') }}</p>
        <NvxField
          for-id="desktop-vault-password"
          :label="t('desktop.vaultPassword')"
          :hint="t('desktop.vaultUnlock')"
        >
          <NvxInput
            id="desktop-vault-password"
            v-model="password"
            type="password"
            autocomplete="off"
            :disabled="pending"
            :maxlength="4096"
            @keydown.enter="decide(true)"
          />
        </NvxField>
      </template>
      <template v-else-if="prompt.prompt.kind === 'vaultCreate'">
        <p>{{ t('desktop.vaultCreate') }}</p>
        <NvxField
          for-id="desktop-vault-password"
          :label="t('desktop.vaultPassword')"
          :hint="t('desktop.vaultMinimum')"
          :error="passwordTooShort ? t('desktop.vaultTooShort') : undefined"
        >
          <NvxInput
            id="desktop-vault-password"
            v-model="password"
            type="password"
            autocomplete="new-password"
            :disabled="pending"
            :maxlength="4096"
            :invalid="passwordTooShort"
            @keydown.enter="decide(true)"
          />
        </NvxField>
        <NvxField
          for-id="desktop-vault-password-confirmation"
          :label="t('desktop.vaultPasswordConfirmation')"
          :hint="t('desktop.vaultConfirmationHint')"
          :error="confirmationMismatch ? t('desktop.vaultMismatch') : undefined"
        >
          <NvxInput
            id="desktop-vault-password-confirmation"
            v-model="passwordConfirmation"
            type="password"
            autocomplete="new-password"
            :disabled="pending"
            :maxlength="4096"
            :invalid="confirmationMismatch"
            @keydown.enter="decide(true)"
          />
        </NvxField>
      </template>
      <template v-else-if="prompt.prompt.kind === 'keyboardInteractive'">
        <h3>{{ prompt.prompt.name }}</h3>
        <p class="desktop-secure__remote">
          {{ prompt.prompt.instruction }}
        </p>
        <NvxField
          v-for="(label, index) in prompt.prompt.prompts"
          :key="index"
          :for-id="`desktop-answer-${index}`"
          :label="label || t('desktop.answer')"
        >
          <NvxInput
            :id="`desktop-answer-${index}`"
            :model-value="answers[index] ?? ''"
            :type="prompt.prompt.echo[index] ? 'text' : 'password'"
            :disabled="pending"
            :maxlength="4096"
            @update:model-value="answers[index] = $event"
          />
        </NvxField>
      </template>
      <template v-else-if="prompt.prompt.kind === 'hostKey'">
        <NvxInlineNotice
          tone="warning"
          :title="t('desktop.hostKey')"
        />
        <dl>
          <dt>{{ t('desktop.address') }}</dt>
          <dd>{{ prompt.prompt.address }}:{{ prompt.prompt.port }}</dd>
          <dt>{{ t('desktop.algorithm') }}</dt>
          <dd>{{ prompt.prompt.algorithm }}</dd>
          <dt>{{ t('desktop.fingerprint') }}</dt>
          <dd><code>{{ prompt.prompt.fingerprint }}</code></dd>
        </dl>
      </template>
      <template v-else-if="prompt.prompt.kind === 'certificate'">
        <NvxInlineNotice
          tone="warning"
          :title="t('desktop.certificate')"
        />
        <dl>
          <dt>{{ t('desktop.address') }}</dt>
          <dd>{{ prompt.prompt.address }}</dd>
          <dt>{{ t('desktop.fingerprint') }}</dt>
          <dd><code>{{ prompt.prompt.fingerprint }}</code></dd>
        </dl>
      </template>
      <template v-else-if="prompt.prompt.kind === 'unencryptedVnc'">
        <p>{{ prompt.prompt.address }}</p>
        <NvxInlineNotice
          tone="warning"
          :title="t('desktop.vncRisk')"
        />
        <p v-if="prompt.prompt.gateway">
          {{ t('desktop.gatewayRisk') }}
        </p>
      </template>
    </div>
    <template
      v-if="prompt && !done"
      #actions
    >
      <NvxButton
        variant="secondary"
        :disabled="pending"
        @click="decide(false)"
      >
        {{ t('desktop.reject') }}
      </NvxButton>
      <NvxButton
        :loading="pending"
        :disabled="!canApprove"
        @click="decide(true)"
      >
        {{ t('desktop.approve') }}
      </NvxButton>
    </template>
  </NvxSecureWindow>
</template>
<style scoped>
.desktop-secure {
  display: grid;
  gap: var(--nvx-space-3);
}

h2,
h3,
p {
  margin: 0;
}

h2 {
  font-size: var(--nvx-font-size-lg);
}

h3 {
  font-size: var(--nvx-font-size-md);
}

dl {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  gap: var(--nvx-space-2) var(--nvx-space-4);
  margin: 0;
}

dt {
  color: var(--nvx-color-text-secondary);
}

dd {
  margin: 0;
  overflow-wrap: anywhere;
}

.desktop-secure__remote {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
</style>
