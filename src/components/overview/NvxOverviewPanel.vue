<script setup lang="ts">
import { Search } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import type { ServerOverviewSnapshot } from "../../core-api/generated/core-api";
import { NvxPluginExtensionTarget } from "../plugins";
import { NvxButton, NvxIcon, NvxInput, NvxInlineNotice, NvxSelect } from "../ui";
import NvxServerCard from "./NvxServerCard.vue";

const props = defineProps<{ snapshot: ServerOverviewSnapshot }>();
const emit = defineEmits<{
  connect: [hostId: string];
  focusTerminal: [sessionId: string];
  metricsAction: [hostId: string];
}>();

const { t } = useI18n();
const query = ref("");
const groupFilter = ref("all");
const statusFilter = ref<"all" | "online" | "issues" | "offline">("all");
const panelElement = ref<HTMLElement | null>(null);
const gridViewportElement = ref<HTMLElement | null>(null);
const cardElements = new Map<string, HTMLElement>();
const viewportWidth = ref(0);
const viewportHeight = ref(0);
const viewportScrollTop = ref(0);
const gridViewportTop = ref(0);
let viewportResizeObserver: ResizeObserver | null = null;
const VIRTUAL_CARD_THRESHOLD = 30;
const VIRTUAL_CARD_MIN_WIDTH = 312;
const VIRTUAL_CARD_HEIGHT = 180;
const VIRTUAL_GRID_GAP = 12;
const VIRTUAL_OVERSCAN_ROWS = 2;
const UNGROUPED_FILTER = "ungrouped";
type CardSection = {
  key: string;
  label: string;
  ungrouped: boolean;
  cards: ServerOverviewSnapshot["cards"];
};
const groupOptions = computed(() => {
  const groups = new Map<string, { label: string; count: number }>();
  let ungroupedCount = 0;
  for (const card of props.snapshot.cards) {
    const group = card.catalogEntry.group;
    if (!group) {
      ungroupedCount += 1;
      continue;
    }
    const current = groups.get(group.groupId);
    groups.set(group.groupId, {
      label: group.label,
      count: (current?.count ?? 0) + 1,
    });
  }
  const options = [...groups.entries()]
    .sort(([, left], [, right]) => left.label.localeCompare(right.label))
    .map(([value, group]) => ({
      value,
      label: t("overview.groupOption", { label: group.label, count: group.count }),
    }));
  if (ungroupedCount > 0) {
    options.push({
      value: UNGROUPED_FILTER,
      label: t("overview.ungroupedOption", { count: ungroupedCount }),
    });
  }
  return [{ value: "all", label: t("overview.allGroups") }, ...options];
});
const statusOptions = computed(() => [
  { value: "all" as const, label: t("overview.statusFilter.all") },
  { value: "online" as const, label: t("overview.statusFilter.online") },
  { value: "issues" as const, label: t("overview.statusFilter.issues") },
  { value: "offline" as const, label: t("overview.statusFilter.offline") },
]);
const summary = computed(() => {
  const all = props.snapshot.cards;
  return {
    total: all.length,
    online: all.filter((card) => ["connected", "monitoringOnly"].includes(card.connectionState)).length,
    issues: all.filter((card) => ["degraded", "failed"].includes(card.connectionState)).length,
  };
});

const cards = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase();
  const filtered = props.snapshot.cards.filter((card) => {
    const entry = card.catalogEntry;
    const matchesQuery = !needle || [
      entry.host.label,
      entry.host.address,
      entry.host.username ?? "",
      entry.group?.label ?? "",
      ...entry.tags.map((tag) => tag.label),
    ].some((value) => value.toLocaleLowerCase().includes(needle));
    const matchesGroup = groupFilter.value === "all"
      || (groupFilter.value === UNGROUPED_FILTER
        ? entry.group === null
        : entry.group?.groupId === groupFilter.value);
    const matchesStatus = statusFilter.value === "all"
      || (statusFilter.value === "online"
        && ["connecting", "connected", "monitoringOnly"].includes(card.connectionState))
      || (statusFilter.value === "issues"
        && ["degraded", "failed"].includes(card.connectionState))
      || (statusFilter.value === "offline" && card.connectionState === "disconnected");
    return matchesQuery && matchesGroup && matchesStatus;
  });
  return [...filtered].sort((left, right) => (
    Number(right.catalogEntry.host.favorite) - Number(left.catalogEntry.host.favorite)
    || left.catalogEntry.host.label.localeCompare(right.catalogEntry.host.label)
  ));
});
const cardSections = computed<CardSection[]>(() => {
  const sections = new Map<string, CardSection>();
  for (const card of cards.value) {
    const group = card.catalogEntry.group;
    const key = group?.groupId ?? UNGROUPED_FILTER;
    const current = sections.get(key);
    if (current) {
      current.cards.push(card);
      continue;
    }
    sections.set(key, {
      key,
      label: group?.label ?? t("overview.ungrouped"),
      ungrouped: group === null,
      cards: [card],
    });
  }
  return [...sections.values()].sort((left, right) => (
    Number(left.ungrouped) - Number(right.ungrouped)
    || left.label.localeCompare(right.label)
  ));
});
watch(groupOptions, (options) => {
  if (!options.some((option) => option.value === groupFilter.value)) groupFilter.value = "all";
});
const virtualized = computed(() => cards.value.length > VIRTUAL_CARD_THRESHOLD);
const columnCount = computed(() => Math.max(1, Math.floor(
  (viewportWidth.value + VIRTUAL_GRID_GAP) / (VIRTUAL_CARD_MIN_WIDTH + VIRTUAL_GRID_GAP),
)));
const rowCount = computed(() => Math.ceil(cards.value.length / columnCount.value));
const visibleStartRow = computed(() => {
  if (!virtualized.value) return 0;
  const relativeScrollTop = Math.max(0, viewportScrollTop.value - gridViewportTop.value);
  return Math.max(0, Math.floor(relativeScrollTop / VIRTUAL_CARD_HEIGHT) - VIRTUAL_OVERSCAN_ROWS);
});
const visibleEndRow = computed(() => {
  if (!virtualized.value) return rowCount.value;
  const relativeScrollTop = Math.max(0, viewportScrollTop.value - gridViewportTop.value);
  const visibleRows = Math.ceil(viewportHeight.value / VIRTUAL_CARD_HEIGHT);
  return Math.min(
    rowCount.value,
    Math.ceil(relativeScrollTop / VIRTUAL_CARD_HEIGHT) + visibleRows + VIRTUAL_OVERSCAN_ROWS,
  );
});
const renderedCards = computed(() => {
  if (!virtualized.value) return cards.value;
  return cards.value.slice(
    visibleStartRow.value * columnCount.value,
    visibleEndRow.value * columnCount.value,
  );
});
const virtualViewportStyle = computed(() => virtualized.value
  ? { height: `${rowCount.value * VIRTUAL_CARD_HEIGHT}px` }
  : undefined);
const virtualWindowStyle = computed(() => virtualized.value
  ? { transform: `translateY(${visibleStartRow.value * VIRTUAL_CARD_HEIGHT}px)` }
  : undefined);

function refreshViewport() {
  const panel = panelElement.value;
  const gridViewport = gridViewportElement.value;
  if (!panel || !gridViewport) return;
  viewportWidth.value = gridViewport.clientWidth || panel.clientWidth || window.innerWidth;
  viewportHeight.value = panel.clientHeight || window.innerHeight;
  viewportScrollTop.value = panel.scrollTop;
  const panelRect = panel.getBoundingClientRect();
  const gridRect = gridViewport.getBoundingClientRect();
  gridViewportTop.value = gridRect.top - panelRect.top + panel.scrollTop;
}

onMounted(() => {
  refreshViewport();
  window.addEventListener("resize", refreshViewport);
  if (typeof ResizeObserver !== "undefined") {
    viewportResizeObserver = new ResizeObserver(refreshViewport);
    if (panelElement.value) viewportResizeObserver.observe(panelElement.value);
    if (gridViewportElement.value) viewportResizeObserver.observe(gridViewportElement.value);
  }
});

onBeforeUnmount(() => {
  window.removeEventListener("resize", refreshViewport);
  viewportResizeObserver?.disconnect();
  viewportResizeObserver = null;
});
watch([cards, virtualized], () => void nextTick(refreshViewport));

function setCardElement(hostId: string, element: Element | null) {
  if (element instanceof HTMLElement) cardElements.set(hostId, element);
  else cardElements.delete(hostId);
}

async function moveCardFocus(hostId: string, direction: 1 | -1) {
  const index = cards.value.findIndex((card) => card.catalogEntry.host.hostId === hostId);
  if (index < 0 || cards.value.length < 2) return;
  const next = cards.value[(index + direction + cards.value.length) % cards.value.length];
  if (!next) return;
  const targetId = next.catalogEntry.host.hostId;
  const rendered = cardElements.get(targetId);
  if (rendered) {
    rendered.focus();
    return;
  }
  const targetIndex = cards.value.findIndex((card) => card.catalogEntry.host.hostId === targetId);
  if (!virtualized.value || targetIndex < 0 || !panelElement.value) return;
  panelElement.value.scrollTop = gridViewportTop.value
    + Math.floor(targetIndex / columnCount.value) * VIRTUAL_CARD_HEIGHT;
  refreshViewport();
  await nextTick();
  cardElements.get(targetId)?.focus();
}

function onCardKeydown(hostId: string, event: KeyboardEvent) {
  if (event.key === "ArrowRight" || event.key === "ArrowDown") {
    event.preventDefault();
    void moveCardFocus(hostId, 1);
  } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
    event.preventDefault();
    void moveCardFocus(hostId, -1);
  }
}
</script>

<template>
  <section
    ref="panelElement"
    class="nvx-overview-panel"
    @scroll.passive="refreshViewport"
  >
    <header class="nvx-overview-panel__topbar">
      <div class="nvx-overview-panel__heading">
        <h1>{{ t('overview.title') }}</h1>
        <p>{{ t('overview.summary', summary) }}</p>
      </div>
      <div class="nvx-overview-panel__toolbar">
        <NvxPluginExtensionTarget
          target-id="overview.toolbar"
          instance-key="global"
        />
        <NvxSelect
          id="overview-group-filter"
          v-model="groupFilter"
          class="nvx-overview-panel__group-filter"
          :options="groupOptions"
          :aria-label="t('overview.groupFilter')"
        />
        <span class="nvx-overview-panel__search-field">
          <NvxIcon
            :icon="Search"
            :size="16"
          />
          <NvxInput
            id="overview-search"
            v-model="query"
            class="nvx-overview-panel__search"
            :aria-label="t('overview.search')"
            :placeholder="t('overview.searchPlaceholder')"
          />
        </span>
        <div
          class="nvx-overview-panel__status-filters"
          role="group"
          :aria-label="t('overview.statusFilterLabel')"
        >
          <NvxButton
            v-for="option in statusOptions"
            :key="option.value"
            size="sm"
            :variant="statusFilter === option.value ? 'secondary' : 'ghost'"
            :aria-pressed="statusFilter === option.value"
            @click="statusFilter = option.value"
          >
            {{ option.label }}
          </NvxButton>
        </div>
      </div>
    </header>

    <NvxInlineNotice v-if="cards.length === 0">
      {{ t('overview.empty') }}
    </NvxInlineNotice>
    <div
      v-else-if="virtualized"
      ref="gridViewportElement"
      class="nvx-overview-panel__grid-viewport nvx-overview-panel__grid-viewport--virtual"
      :style="virtualViewportStyle"
    >
      <div
        class="nvx-overview-panel__grid nvx-overview-panel__grid--virtual"
        :style="virtualWindowStyle"
      >
        <div
          v-for="card in renderedCards"
          :key="card.catalogEntry.host.hostId"
          :ref="(element) => setCardElement(card.catalogEntry.host.hostId, element as Element | null)"
          class="nvx-overview-panel__card"
          tabindex="0"
          @keydown="onCardKeydown(card.catalogEntry.host.hostId, $event)"
        >
          <NvxServerCard
            :card="card"
            @connect="emit('connect', $event)"
            @focus-terminal="emit('focusTerminal', $event)"
            @metrics-action="emit('metricsAction', $event)"
          />
        </div>
      </div>
    </div>
    <div
      v-else
      class="nvx-overview-panel__sections"
    >
      <section
        v-for="section in cardSections"
        :key="section.key"
        class="nvx-overview-panel__section"
        :aria-labelledby="`overview-group-${section.key}`"
      >
        <header class="nvx-overview-panel__section-header">
          <h2 :id="`overview-group-${section.key}`">
            {{ t("overview.groupSection", { label: section.label, count: section.cards.length }) }}
          </h2>
        </header>
        <div class="nvx-overview-panel__grid">
          <div
            v-for="card in section.cards"
            :key="card.catalogEntry.host.hostId"
            :ref="(element) => setCardElement(card.catalogEntry.host.hostId, element as Element | null)"
            class="nvx-overview-panel__card"
            tabindex="0"
            @keydown="onCardKeydown(card.catalogEntry.host.hostId, $event)"
          >
            <NvxServerCard
              :card="card"
              @connect="emit('connect', $event)"
              @focus-terminal="emit('focusTerminal', $event)"
              @metrics-action="emit('metricsAction', $event)"
            />
          </div>
        </div>
      </section>
    </div>
  </section>
</template>

<style scoped>
.nvx-overview-panel {
  height: 100%;
  overflow: auto;
  padding: var(--nvx-space-4);
}

.nvx-overview-panel__topbar {
  display: flex;
  gap: var(--nvx-space-4);
  align-items: center;
  justify-content: space-between;
  min-height: 64px;
  margin-bottom: var(--nvx-space-3);
  padding: var(--nvx-space-2) 0;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
}

.nvx-overview-panel__heading {
  flex: 0 0 auto;
  min-width: 164px;
}

.nvx-overview-panel__heading h1,
.nvx-overview-panel__heading p {
  margin: 0;
}

.nvx-overview-panel__heading h1 {
  font-size: var(--nvx-font-size-lg);
  line-height: var(--nvx-line-height-lg);
}

.nvx-overview-panel__heading p {
  margin-top: 2px;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
  white-space: nowrap;
}

.nvx-overview-panel__toolbar {
  display: grid;
  grid-template-columns: minmax(144px, 180px) minmax(220px, 280px) max-content;
  gap: var(--nvx-space-2);
  align-items: center;
  min-width: 0;
}

.nvx-overview-panel__search-field {
  position: relative;
  display: flex;
  align-items: center;
  min-width: 0;
}

.nvx-overview-panel__search-field > svg {
  position: absolute;
  z-index: 1;
  left: var(--nvx-space-3);
  color: var(--nvx-color-text-tertiary);
  pointer-events: none;
}

.nvx-overview-panel__search {
  min-width: 0;
  padding-inline-start: 34px;
}

.nvx-overview-panel__status-filters {
  display: flex;
  gap: 2px;
  align-items: center;
  min-width: 0;
}

.nvx-overview-panel__group-filter {
  min-width: 0;
}

.nvx-overview-panel__grid-viewport {
  position: relative;
}

.nvx-overview-panel__sections {
  display: grid;
  gap: var(--nvx-space-5);
}

.nvx-overview-panel__section {
  display: grid;
  gap: var(--nvx-space-3);
}

.nvx-overview-panel__section-header {
  display: flex;
  align-items: center;
  min-height: 28px;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
}

.nvx-overview-panel__section-header h2 {
  margin: 0;
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
  font-weight: var(--nvx-font-weight-semibold);
}

.nvx-overview-panel__grid {
  --nvx-overview-card-min-width: 312px;
  --nvx-overview-card-width: 328px;

  display: grid;
  grid-template-columns: repeat(
    auto-fill,
    minmax(min(100%, var(--nvx-overview-card-min-width)), var(--nvx-overview-card-width))
  );
  gap: var(--nvx-space-3);
  align-items: start;
  justify-content: start;
}

.nvx-overview-panel__grid--virtual {
  position: absolute;
  top: 0;
  right: 0;
  left: 0;
}

.nvx-overview-panel__grid-viewport--virtual .nvx-overview-panel__card {
  height: calc(180px - var(--nvx-space-3));
  content-visibility: visible;
}

.nvx-overview-panel__grid-viewport--virtual .nvx-overview-panel__card :deep(.nvx-server-card) {
  height: 100%;
}

.nvx-overview-panel__card {
  content-visibility: auto;
  contain-intrinsic-size: auto 168px;
  border-radius: var(--nvx-radius-md);
}

.nvx-overview-panel__card:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 2px;
}

@media (max-width: 1180px) {
  .nvx-overview-panel__topbar {
    align-items: flex-start;
    flex-direction: column;
  }

  .nvx-overview-panel__toolbar {
    width: 100%;
    grid-template-columns: minmax(144px, 180px) minmax(220px, 1fr) max-content;
  }
}

@media (max-width: 760px) {
  .nvx-overview-panel__toolbar {
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.4fr);
  }

  .nvx-overview-panel__status-filters {
    grid-column: 1 / -1;
    flex-wrap: wrap;
  }
}
</style>
