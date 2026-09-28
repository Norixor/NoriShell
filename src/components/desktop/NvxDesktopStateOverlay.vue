<script setup lang="ts">
import { CircleAlert, LoaderCircle, MonitorOff, RotateCw } from "lucide-vue-next";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { DesktopSessionSummary } from "../../core-api/generated/core-api";
import { NvxButton, NvxIcon } from "../ui";

const props = defineProps<{ session: DesktopSessionSummary; busy: boolean }>();
const emit = defineEmits<{ reconnect: [] }>();
const { t, te } = useI18n();

/** Running sessions show the live canvas; every other Core state is explained in place of a stale image. */
const kind = computed(() => {
  switch (props.session.state) {
    case "connecting":
    case "needsInteraction":
    case "disconnecting":
      return "progress";
    case "failed":
      return "failed";
    case "closed":
      return "closed";
    default:
      return null;
  }
});
const title = computed(() => {
  const { state, phase } = props.session;
  if (state === "connecting" && te(`desktop.phases.${phase}`)) return t(`desktop.phases.${phase}`);
  return t(`desktop.states.${state}`);
});
const detail = computed(() => {
  const { state, failure } = props.session;
  if (state === "needsInteraction") return t("desktop.needsInteractionHint");
  if (state === "failed") return failure && te(`desktop.errors.${failure}`) ? t(`desktop.errors.${failure}`) : t("desktop.failure");
  if (state === "closed") return t("desktop.closedHint");
  return "";
});
</script>

<template>
  <div
    v-if="kind"
    class="desktop-state"
    :class="`desktop-state--${kind}`"
    role="status"
    aria-live="polite"
  >
    <div class="desktop-state__card">
      <NvxIcon
        v-if="kind === 'progress'"
        class="desktop-state__spinner"
        :icon="LoaderCircle"
        :size="22"
      />
      <NvxIcon
        v-else
        class="desktop-state__icon"
        :icon="kind === 'failed' ? CircleAlert : MonitorOff"
        :size="22"
      />
      <strong class="desktop-state__title">{{ title }}</strong>
      <p
        v-if="detail"
        class="desktop-state__detail"
      >
        {{ detail }}
      </p>
      <NvxButton
        v-if="kind !== 'progress'"
        :variant="kind === 'failed' ? 'primary' : 'secondary'"
        :disabled="busy"
        @click="emit('reconnect')"
      >
        <NvxIcon
          :icon="RotateCw"
          :size="16"
        />{{ t('desktop.reconnect') }}
      </NvxButton>
    </div>
  </div>
</template>

<style scoped>
.desktop-state {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  padding: var(--nvx-space-6);
  background: color-mix(in srgb, var(--nvx-color-terminal-bg) 72%, transparent);
}

.desktop-state__card {
  display: grid;
  gap: var(--nvx-space-3);
  justify-items: center;
  max-width: 360px;
  padding: var(--nvx-space-5) var(--nvx-space-6);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-overlay);
  text-align: center;
}

.desktop-state__spinner {
  color: var(--nvx-color-accent);
}

.desktop-state__spinner :deep(svg) {
  animation: desktop-state-spin 0.8s linear infinite;
}

.desktop-state__icon {
  color: var(--nvx-color-text-secondary);
}

.desktop-state--failed .desktop-state__icon {
  color: var(--nvx-color-danger);
}

.desktop-state__title {
  font-size: var(--nvx-font-size-md);
  font-weight: var(--nvx-font-weight-semibold);
}

.desktop-state__detail {
  margin: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
}

@keyframes desktop-state-spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .desktop-state__spinner :deep(svg) {
    animation: none;
  }
}
</style>
