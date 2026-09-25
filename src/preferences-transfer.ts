// Transfer files contain only explicitly registered global non-secret preferences, never localStorage or runtime state.
export const PREFERENCE_TRANSFER_VERSION = 1;
export const MAX_PREFERENCE_TRANSFER_BYTES = 256 * 1024;
export const PREFERENCE_GROUP_IDS = ["application", "appearance", "interaction", "highlights", "shortcuts", "files", "desktop", "commandNotifications"] as const;
export type PreferenceGroupId = typeof PREFERENCE_GROUP_IDS[number];
export type PreferenceGroupResult = "pending" | "applying" | "applied" | "unchanged" | "conflict" | "failed";

export interface PreferenceGroupAdapter {
  id: PreferenceGroupId;
  validate(value: unknown): boolean;
  read(): Promise<unknown>;
  apply(value: unknown, expected: unknown): Promise<boolean>;
  defaults(): unknown;
}

export interface PreferencePreviewGroup {
  readonly id: PreferenceGroupId;
  readonly before: string;
  readonly after: string;
  result: PreferenceGroupResult;
}

export interface PreferenceTransferFile {
  product: "NoriShell";
  version: 1;
  groups: Partial<Record<PreferenceGroupId, unknown>>;
}

export function validateFullSyncPreferences(value: unknown, adapters: readonly PreferenceGroupAdapter[]): PreferenceTransferFile {
  if (!object(value)) throw new Error("invalidFile");
  const parsed = parsePreferenceTransfer(JSON.stringify(value), adapters);
  if (adapters.length !== PREFERENCE_GROUP_IDS.length
    || PREFERENCE_GROUP_IDS.some((id) => !adapters.some((adapter) => adapter.id === id) || !(id in parsed.groups))
    || Object.keys(parsed.groups).length !== PREFERENCE_GROUP_IDS.length) throw new Error("incompletePreferences");
  return parsed;
}

export async function exportFullSyncPreferences(adapters: readonly PreferenceGroupAdapter[]): Promise<PreferenceTransferFile> {
  return validateFullSyncPreferences(JSON.parse(await exportPreferenceTransfer(adapters)), adapters);
}

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function canonical(value: unknown): string {
  return JSON.stringify(value, (_key, item: unknown) => object(item)
    ? Object.fromEntries(Object.keys(item).sort().map((key) => [key, item[key]]))
    : item);
}

export function preferenceValuesEqual(left: unknown, right: unknown): boolean {
  return canonical(left) === canonical(right);
}

export function parsePreferenceTransfer(text: string, adapters: readonly PreferenceGroupAdapter[]): PreferenceTransferFile {
  if (new TextEncoder().encode(text).byteLength > MAX_PREFERENCE_TRANSFER_BYTES) throw new Error("tooLarge");
  const file: unknown = JSON.parse(text);
  if (!object(file) || Object.keys(file).some((key) => !["product", "version", "groups"].includes(key))
    || file.product !== "NoriShell" || file.version !== PREFERENCE_TRANSFER_VERSION || !object(file.groups)
    || Object.keys(file.groups).length === 0) throw new Error("invalidFile");
  for (const [id, value] of Object.entries(file.groups)) {
    const adapter = adapters.find((item) => item.id === id);
    if (!adapter || !adapter.validate(value)) throw new Error("invalidGroup");
  }
  return file as unknown as PreferenceTransferFile;
}

export async function exportPreferenceTransfer(adapters: readonly PreferenceGroupAdapter[]): Promise<string> {
  if (adapters.length === 0) throw new Error("emptySelection");
  const groups: PreferenceTransferFile["groups"] = {};
  for (const adapter of adapters) {
    const value = await adapter.read();
    if (!adapter.validate(value)) throw new Error("unavailableGroup");
    groups[adapter.id] = value;
  }
  const text = JSON.stringify({ product: "NoriShell", version: PREFERENCE_TRANSFER_VERSION, groups }, null, 2);
  if (new TextEncoder().encode(text).byteLength > MAX_PREFERENCE_TRANSFER_BYTES) throw new Error("tooLarge");
  return text;
}

export async function previewPreferenceTransfer(file: PreferenceTransferFile, adapters: readonly PreferenceGroupAdapter[]): Promise<PreferencePreviewGroup[]> {
  const preview: PreferencePreviewGroup[] = [];
  for (const [id, value] of Object.entries(file.groups)) {
    const adapter = adapters.find((item) => item.id === id);
    if (!adapter || !adapter.validate(value)) throw new Error("invalidGroup");
    const before = await adapter.read();
    if (!adapter.validate(before)) throw new Error("unavailableGroup");
    // A string snapshot isolates later file or form edits; callers cannot replace the preview with a new pending object.
    const group = { result: "pending" as PreferenceGroupResult } as PreferencePreviewGroup;
    Object.defineProperties(group, {
      id: { value: adapter.id, enumerable: true },
      before: { value: canonical(before), enumerable: true },
      after: { value: canonical(value), enumerable: true },
    });
    preview.push(group);
  }
  return preview;
}

export async function applyPreferencePreview(preview: readonly PreferencePreviewGroup[], selected: ReadonlySet<PreferenceGroupId>, adapters: readonly PreferenceGroupAdapter[], shouldContinue: () => boolean = () => true): Promise<void> {
  for (const group of preview) {
    if (!shouldContinue()) break;
    // Attempted groups never replay automatically; failures and conflicts require a new preview before explicit application.
    if (!selected.has(group.id) || group.result !== "pending") continue;
    group.result = "applying";
    const adapter = adapters.find((item) => item.id === group.id);
    if (!adapter) { group.result = "failed"; continue; }
    try {
      const current = await adapter.read();
      if (!shouldContinue()) { group.result = "failed"; break; }
      if (canonical(current) !== group.before) { group.result = "conflict"; continue; }
      if (group.before === group.after) { group.result = "unchanged"; continue; }
      const next: unknown = JSON.parse(group.after);
      if (!adapter.validate(next)) { group.result = "failed"; continue; }
      group.result = await adapter.apply(next, current) ? "applied" : "failed";
    } catch { group.result = "failed"; }
  }
}
