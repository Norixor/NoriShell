import { invoke, isTauri } from "@tauri-apps/api/core";

import type {
  ApplicationPreferenceGroupId,
  ApplicationPreferencesSnapshot,
} from "./generated/core-api";
import { preferenceValuesEqual } from "../preferences-transfer";
import { parseCoreApiError } from "./client";

export type ApplicationPreferenceFailure =
  | "conflict" | "invalidInput" | "persistenceUnavailable" | "unsupportedSchema" | "requiresReconciliation" | "unknown";

class ApplicationPreferenceClientError extends Error {
  constructor(readonly code: string, readonly messageKey: string) { super(code); }
}

export function applicationPreferenceFailure(error: unknown): ApplicationPreferenceFailure {
  switch (parseCoreApiError(error)?.code) {
    case "application_preferences.conflict": return "conflict";
    case "application_preferences.invalid_input": return "invalidInput";
    case "application_preferences.persistence_unavailable": return "persistenceUnavailable";
    case "application_preferences.unsupported_schema": return "unsupportedSchema";
    case "application_preferences.requires_reconciliation": return "requiresReconciliation";
    default: return "unknown";
  }
}

const snapshots = new Map<ApplicationPreferenceGroupId, ApplicationPreferencesSnapshot>();

function meta() { return { requestId: crypto.randomUUID() }; }

export function corePreferencesEnabled() { return isTauri(); }

export function getApplicationPreferences(group: ApplicationPreferenceGroupId): Promise<ApplicationPreferencesSnapshot> {
  return invoke("application_preferences_get", { request: { meta: meta(), group } });
}

export function replaceApplicationPreferences(
  group: ApplicationPreferenceGroupId,
  value: unknown,
  expectedRevision: string | null,
): Promise<ApplicationPreferencesSnapshot> {
  return invoke("application_preferences_replace", { request: { meta: meta(), group, expectedRevision, value } });
}

export function installApplicationPreferencesSnapshot(snapshot: ApplicationPreferencesSnapshot) {
  snapshots.set(snapshot.group, snapshot);
}

export function currentApplicationPreferencesValue(group: ApplicationPreferenceGroupId): unknown | null {
  return snapshots.get(group)?.value ?? null;
}

export async function saveApplicationPreferences(
  group: ApplicationPreferenceGroupId,
  value: unknown,
  expectedValue: unknown,
): Promise<boolean> {
  const previous = snapshots.get(group);
  if (!previous || !preferenceValuesEqual(previous.value, expectedValue)) {
    throw new ApplicationPreferenceClientError("application_preferences.conflict", "errors.applicationPreferences.conflict");
  }
  const next = await replaceApplicationPreferences(group, value, previous.revision);
  if (next.group !== group || !preferenceValuesEqual(next.value, value) || next.revision === null) {
    throw new ApplicationPreferenceClientError("application_preferences.requires_reconciliation", "errors.applicationPreferences.requiresReconciliation");
  }
  snapshots.set(group, next);
  return true;
}
