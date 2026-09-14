<script setup lang="ts">
import { onBeforeUnmount, onMounted } from "vue";
import { useI18n } from "vue-i18n";
import { NvxButton, NvxCheckbox } from "../ui";
import { usePluginAppIntegrationsStore } from "../../stores/pluginAppIntegrations";
import { useTipsStore } from "../../stores/tips";
import type { PluginAppCommand, PluginAppIntegrationSnapshot } from "../../core-api/generated/core-api";
withDefaults(defineProps<{ commandsOnly?: boolean }>(), { commandsOnly: false });
const { t } = useI18n();
const integrations = usePluginAppIntegrationsStore();
const tips = useTipsStore();
let timer: ReturnType<typeof setInterval> | undefined;
let alive = true;
async function refresh() { try { await integrations.refresh(); } catch { /* Next bounded poll refreshes the projection. */ } }
async function run(snapshot: PluginAppIntegrationSnapshot, command: PluginAppCommand) {
  try { await integrations.run(snapshot, command); }
  catch { if (alive) tips.show({ tone: "error", title: t("pluginAppIntegrations.error") }); }
}
onMounted(() => { void refresh(); timer = setInterval(() => { void refresh(); }, 3000); });
onBeforeUnmount(() => { alive = false; if (timer) clearInterval(timer); });
</script>
<template>
  <section
    v-if="integrations.snapshots.length"
    class="plugin-app-integrations"
    data-plugin-protected
  >
    <header>
      <strong>{{ t("pluginAppIntegrations.title") }}</strong><NvxCheckbox
        v-if="!commandsOnly"
        v-model="integrations.enabledBindings"
      >
        {{ t("pluginAppIntegrations.bindings") }}
      </NvxCheckbox>
    </header>
    <p
      v-if="!commandsOnly"
      class="hint"
    >
      {{ t("pluginAppIntegrations.hint") }}
    </p>
    <article
      v-for="snapshot in integrations.snapshots"
      :key="`${snapshot.pluginId}:${snapshot.instanceGeneration}`"
    >
      <strong>{{ snapshot.pluginName }}</strong>
      <span
        v-for="status in commandsOnly ? [] : snapshot.registration.statuses"
        :key="status.id"
        role="status"
      >{{ status.text }}</span>
      <div
        v-for="command in snapshot.registration.commands"
        :key="command.id"
        class="command"
      >
        <span>{{ command.label }} <small v-if="command.fileExtensions.length">.{{ command.fileExtensions.join(' · .') }}</small></span>
        <kbd v-if="command.shortcut">{{ command.shortcut }}</kbd>
        <NvxButton
          size="sm"
          variant="secondary"
          :disabled="integrations.busy"
          @click="run(snapshot, command)"
        >
          {{ t(command.fileExtensions.length ? "pluginAppIntegrations.open" : "pluginAppIntegrations.run") }}
        </NvxButton>
      </div>
      <p
        v-for="notification in commandsOnly ? [] : snapshot.notifications"
        :key="notification.id"
        role="status"
      >
        {{ notification.text }}
      </p>
    </article>
  </section>
</template>
<style scoped>
.plugin-app-integrations { display: grid; gap: 8px; padding: 12px; border: 1px solid var(--nvx-color-border-subtle); border-radius: 8px; }
header, .command { display: flex; align-items: center; gap: 10px; justify-content: space-between; }
article { display: grid; gap: 6px; padding-block: 6px; }
p { margin: 0; }
.hint, small, kbd { color: var(--nvx-color-text-muted); font-size: 12px; }
.command > span { flex: 1; }
</style>
