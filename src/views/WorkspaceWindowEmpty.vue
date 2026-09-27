<script setup lang="ts">
import { onMounted } from "vue";
import { useI18n } from "vue-i18n";
import { useRouteReveal } from "../routeReveal";

const { t } = useI18n();
const revealRoute = useRouteReveal();
onMounted(revealRoute);
</script>

<template>
  <section class="workspace-window-empty">
    <p>{{ t("workspaceTabs.emptyWindow") }}</p>
  </section>
</template>

<style scoped>
.workspace-window-empty {
  display: grid;
  flex: 1 1 auto;
  place-items: center;
  min-width: 0;
  min-height: 0;
  color: var(--nvx-color-text-secondary);
}
</style>
