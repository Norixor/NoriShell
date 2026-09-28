import type { MetricSnapshot, ServerOverviewCard } from "../../core-api/generated/core-api";

export type MetricPercents = { cpu: number | null; memory: number | null; disk: number | null };
export type MetricHistory = { cpu: number[]; memory: number[]; disk: number[] };
type MetricHistoryEntry = MetricHistory & { streamKey: string; sampleSequence: string };
export type MetricHistoryMap = ReadonlyMap<string, MetricHistoryEntry>;

export const METRIC_HISTORY_LIMIT = 24;

export function ratioPercent(used: string | null | undefined, total: string | null | undefined) {
  if (used == null || total == null) return null;
  const denominator = BigInt(total);
  if (denominator === 0n) return null;
  return Number((BigInt(used) * 10_000n) / denominator) / 100;
}

export function metricPercents(snapshot: MetricSnapshot): MetricPercents {
  const disk = snapshot.disks[0];
  return {
    cpu: snapshot.cpu.basisPoints == null ? null : snapshot.cpu.basisPoints / 100,
    memory: ratioPercent(snapshot.memory.usedBytes, snapshot.memory.totalBytes),
    disk: ratioPercent(disk?.usedBytes, disk?.totalBytes),
  };
}

/** Only fresh, per-field available readings; stale or failed samples never enter trends or aggregates. */
export function freshMetricPercents(card: ServerOverviewCard): MetricPercents | null {
  const session = card.metricsSession;
  const latest = session?.latestSnapshot;
  if (!card.monitoringPolicy.policy.enabled || !latest || latest.stale || session?.failureCode) return null;
  const percents = metricPercents(latest);
  return {
    cpu: latest.cpu.state === "available" ? percents.cpu : null,
    memory: latest.memory.state === "available" ? percents.memory : null,
    disk: latest.disks[0]?.state === "available" ? percents.disk : null,
  };
}

function appendBounded(series: number[], value: number | null) {
  if (value === null) return series;
  const next = [...series, value];
  return next.length > METRIC_HISTORY_LIMIT ? next.slice(next.length - METRIC_HISTORY_LIMIT) : next;
}

/**
 * Rebuilds the in-memory trend window from the latest Overview projection. A new MetricsSession or
 * generation starts an empty window so samples never mix across connections; nothing is persisted.
 */
export function nextMetricHistory(
  previous: MetricHistoryMap,
  cards: readonly ServerOverviewCard[],
): Map<string, MetricHistoryEntry> {
  const next = new Map<string, MetricHistoryEntry>();
  for (const card of cards) {
    const hostId = card.catalogEntry.host.hostId;
    const latest = card.metricsSession?.latestSnapshot;
    if (!card.monitoringPolicy.policy.enabled || !latest) continue;
    const streamKey = `${latest.metricsSessionId}:${latest.generation}`;
    const current = previous.get(hostId);
    const base = current?.streamKey === streamKey
      ? current
      : { streamKey, sampleSequence: "", cpu: [], memory: [], disk: [] };
    const fresh = freshMetricPercents(card);
    if (!fresh || base.sampleSequence === latest.sampleSequence) {
      next.set(hostId, base);
      continue;
    }
    next.set(hostId, {
      streamKey,
      sampleSequence: latest.sampleSequence,
      cpu: appendBounded(base.cpu, fresh.cpu),
      memory: appendBounded(base.memory, fresh.memory),
      disk: appendBounded(base.disk, fresh.disk),
    });
  }
  return next;
}
