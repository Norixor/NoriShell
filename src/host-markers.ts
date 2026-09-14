export const HOST_MARKER_COLORS = ["red", "amber", "green", "blue", "purple", "neutral"] as const;
export type HostMarkerColor = typeof HOST_MARKER_COLORS[number];
export type HostMarkerPreset = "production" | "testing" | "development";
export type HostMarker = { kind: HostMarkerPreset; color: HostMarkerColor } | {
  kind: "custom";
  label: string;
  color: HostMarkerColor;
};
export const MAX_HOST_MARKER_LENGTH = 12;
export const HOST_MARKER_PRESETS: Record<HostMarkerPreset, HostMarker> = {
  production: { kind: "production", color: "red" },
  testing: { kind: "testing", color: "amber" },
  development: { kind: "development", color: "green" },
};
export function normalizeHostMarker(input: unknown): HostMarker | null {
  if (!input || typeof input !== "object") return null;
  const value = input as Record<string, unknown>;
  if (!HOST_MARKER_COLORS.includes(value.color as HostMarkerColor)) return null;
  const color = value.color as HostMarkerColor;
  if (value.kind === "production" || value.kind === "testing" || value.kind === "development") {
    return { kind: value.kind, color };
  }
  if (value.kind !== "custom" || typeof value.label !== "string") return null;
  const label = value.label.trim();
  if (!label || [...label].length > MAX_HOST_MARKER_LENGTH || /[\p{Cc}\p{Cf}]/u.test(label)) return null;
  return { kind: "custom", label, color };
}
