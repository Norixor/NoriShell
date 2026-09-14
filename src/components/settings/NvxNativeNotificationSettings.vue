<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import {
  getNativeNotificationPermission,
  nativeNotificationFailure,
  requestNativeNotificationPermission,
  testNativeNotification,
  type NativeNotificationFailure,
  type NativeNotificationPermissionSnapshot,
} from "../../core-api/native-notifications";
import { NvxButton, NvxInlineNotice, NvxStatusLabel } from "../ui";

const { t, locale } = useI18n();
const snapshot = ref<NativeNotificationPermissionSnapshot | null>(null);
const busy = ref<"refresh" | "request" | "test" | null>(null);
const error = ref<NativeNotificationFailure | null>(null);
const testAccepted = ref(false);
const testSuppressed = ref(false);
let mounted = false;

const permission = computed(() => snapshot.value?.permission);
const tone = computed(() => permission.value === "granted" ? "success" : permission.value === "denied" ? "warning" : "neutral");
const hint = computed(() => {
  if (permission.value === "unavailable") return "unavailableHint";
  if (permission.value === "denied") return "deniedHint";
  if (permission.value === "granted") return "grantedHint";
  return null;
});

async function perform(operation: "refresh" | "request" | "test") {
  if (busy.value) return;
  busy.value = operation;
  error.value = null;
  testAccepted.value = false;
  testSuppressed.value = false;
  try {
    const result = operation === "refresh"
      ? await getNativeNotificationPermission()
      : operation === "request"
        ? await requestNativeNotificationPermission()
        : await testNativeNotification(locale.value === "en" ? "en" : "zh-CN");
    if (!mounted) return;
    snapshot.value = result;
    testAccepted.value = operation === "test" && result.lastDelivery === "accepted";
    testSuppressed.value = operation === "test" && result.lastDelivery === "suppressed";
  } catch (caught) {
    if (mounted) {
      error.value = nativeNotificationFailure(caught);
      if (operation === "refresh" || error.value === "permissionDenied" || error.value === "unavailable") snapshot.value = null;
    }
  } finally {
    if (mounted) busy.value = null;
  }
}

function refresh() { void perform("refresh"); }
function requestPermission() { void perform("request"); }
function sendTest() { void perform("test"); }

onMounted(() => {
  mounted = true;
  refresh();
  window.addEventListener("focus", refresh);
});
onBeforeUnmount(() => {
  mounted = false;
  window.removeEventListener("focus", refresh);
});

defineExpose({ refresh });
</script>

<template>
  <section
    class="notification-settings"
    aria-labelledby="native-notification-permission-title"
  >
    <div class="notification-settings__heading">
      <div>
        <h3 id="native-notification-permission-title">
          {{ t('nativeNotifications.title') }}
        </h3>
        <p>{{ t('nativeNotifications.description') }}</p>
      </div>
      <NvxButton
        variant="ghost"
        size="sm"
        :loading="busy === 'refresh'"
        :disabled="busy !== null"
        @click="refresh"
      >
        {{ t('nativeNotifications.refresh') }}
      </NvxButton>
    </div>
    <p
      v-if="!snapshot && busy"
      role="status"
    >
      {{ t('nativeNotifications.loading') }}
    </p>
    <template v-if="snapshot">
      <NvxStatusLabel :tone="tone">
        {{ t(`nativeNotifications.permission.${snapshot.permission}`) }}
      </NvxStatusLabel>
      <p v-if="hint">
        {{ t(`nativeNotifications.${hint}`) }}
      </p>
    </template>
    <div class="notification-settings__actions">
      <NvxButton
        v-if="permission === 'notDetermined'"
        variant="secondary"
        size="sm"
        :loading="busy === 'request'"
        :disabled="busy !== null"
        @click="requestPermission"
      >
        {{ t('nativeNotifications.request') }}
      </NvxButton>
      <NvxButton
        variant="secondary"
        size="sm"
        :loading="busy === 'test'"
        :disabled="busy !== null || permission !== 'granted'"
        @click="sendTest"
      >
        {{ t('nativeNotifications.test') }}
      </NvxButton>
    </div>
    <NvxInlineNotice
      v-if="error"
      tone="error"
    >
      {{ t(`nativeNotifications.errors.${error}`) }}
    </NvxInlineNotice>
    <NvxInlineNotice
      v-else-if="testAccepted"
      tone="info"
    >
      {{ t('nativeNotifications.testAccepted') }}
    </NvxInlineNotice>
    <NvxInlineNotice
      v-else-if="testSuppressed"
      tone="info"
    >
      {{ t('nativeNotifications.testSuppressed') }}
    </NvxInlineNotice>
  </section>
</template>

<style scoped>
.notification-settings { display: grid; gap: var(--nvx-space-2); min-width: 0; padding: var(--nvx-space-3) 0; }
.notification-settings__heading { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--nvx-space-3); }
.notification-settings__heading > div { min-width: 0; }
.notification-settings h3 { margin: 0; font-size: var(--nvx-font-size-sm); font-weight: 600; }
.notification-settings p { margin: 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); line-height: 1.5; }
.notification-settings__heading p { margin-top: var(--nvx-space-1); }
.notification-settings__actions { display: flex; flex-wrap: wrap; gap: var(--nvx-space-2); }
@media (max-width: 760px) { .notification-settings__heading { flex-wrap: wrap; } }
</style>
