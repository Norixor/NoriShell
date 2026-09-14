export interface PluginFloatingPreference { x: number; y: number; hidden: boolean }
export interface PluginFloatingPosition { x: number; y: number }
export const floatingButtonSize = 44;
export const floatingButtonGap = 8;

export function placePluginFloatingButtons(
  ids: readonly string[], preferences: Record<string, PluginFloatingPreference>, width: number, height: number,
): Record<string, PluginFloatingPosition> {
  const result: Record<string, PluginFloatingPosition> = {};
  const maximumX = Math.max(0, width - floatingButtonSize - 12);
  const maximumY = Math.max(0, height - floatingButtonSize - 12);
  const distance = floatingButtonSize + floatingButtonGap;
  for (const [index, id] of ids.entries()) {
    const saved = preferences[id];
    let x = Math.max(0, Math.min(maximumX, saved ? saved.x * maximumX : maximumX));
    let y = Math.max(0, Math.min(maximumY, saved ? saved.y * maximumY : maximumY * .45 + index * distance));
    for (let attempt = 0; attempt < 64 && Object.values(result).some((other) => Math.abs(other.x - x) < distance && Math.abs(other.y - y) < distance); attempt += 1) {
      y += distance;
      if (y > maximumY) { y = Math.min(12, maximumY); x = Math.max(0, x - distance); }
    }
    result[id] = { x, y };
  }
  return result;
}

export function readPluginFloatingPreferences(key: string): Record<string, PluginFloatingPreference> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    return Object.fromEntries(Object.entries(parsed).slice(0, 64).flatMap(([id, value]) => {
      if (!value || typeof value !== "object") return [];
      const entry = value as Partial<PluginFloatingPreference>;
      return typeof entry.x === "number" && Number.isFinite(entry.x) && entry.x >= 0 && entry.x <= 1
        && typeof entry.y === "number" && Number.isFinite(entry.y) && entry.y >= 0 && entry.y <= 1 && typeof entry.hidden === "boolean"
        ? [[id, { x: entry.x, y: entry.y, hidden: entry.hidden }]] : [];
    }));
  } catch { return {}; }
}
