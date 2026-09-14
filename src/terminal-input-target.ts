import { computed, ref } from "vue";

import {
  changeTerminalInputFocus,
  fetchTerminalInputFocusSnapshot,
  parseCoreApiError,
} from "./core-api/client";
import type {
  SshLoginAutomationTakeoverResponse,
  TerminalInputFocusChangeResponse,
  TerminalInputFocusTarget,
  TerminalInputLease,
} from "./core-api/generated/core-api";

export type FocusedTerminalRunResult = "sent" | "unavailable" | "failed";

export interface FocusedTerminalInputTarget {
  id: string;
  label(): string;
  focusTarget(): TerminalInputFocusTarget | null;
  applyFocusLease(lease: TerminalInputLease | null): void;
  canAcceptInput(): boolean;
  canAcceptRawInput?(): boolean;
  send(value: string): Promise<void>;
}

export function takeoverTerminalInputTarget(
  id: string,
  takeover: (expectedFocusEpoch: string) => Promise<SshLoginAutomationTakeoverResponse>,
): Promise<SshLoginAutomationTakeoverResponse> {
  const normalizedId = targets.has(id) ? id : null;
  const registration = normalizedId ? targets.get(normalizedId) ?? null : null;
  desiredTargetId.value = normalizedId;
  focusPending.value = true;
  const intentSequence = ++focusIntentSequence;
  const run = async () => {
    if (!normalizedId) throw new Error("Terminal input target is unavailable");
    await ensureFocusEpoch();
    try {
      const response = await takeover(focusEpoch as string);
      focusEpoch = response.focus.focusEpoch;
      if (intentSequence === focusIntentSequence) {
        applyAcknowledgedFocus(normalizedId, {
          focusEpoch: response.focus.focusEpoch,
          target: response.focus.target
            ? { kind: "ssh", target: response.focus.target }
            : null,
          lease: response.focus.lease
            ? { kind: "ssh", lease: response.focus.lease }
            : null,
        }, registration);
        desiredTargetId.value = normalizedId;
        focusPending.value = false;
      }
      return response;
    } catch (error) {
      if (intentSequence === focusIntentSequence) {
        for (const registered of targets.values()) registered.applyFocusLease(null);
        acknowledgedTargetId.value = null;
        acknowledgedRegistration = null;
        acknowledgedTargetFingerprint = null;
        focusPending.value = false;
        registryRevision.value += 1;
      }
      throw error;
    }
  };
  const result = focusQueue.then(run, run);
  focusQueue = result.then(() => undefined, () => undefined);
  return result;
}

const targets = new Map<string, FocusedTerminalInputTarget>();
const desiredTargetId = ref<string | null>(null);
const acknowledgedTargetId = ref<string | null>(null);
const focusPending = ref(false);
const registryRevision = ref(0);
let focusEpoch: string | null = null;
let acknowledgedRegistration: FocusedTerminalInputTarget | null = null;
let acknowledgedTargetFingerprint: string | null = null;
let focusIntentSequence = 0;
let focusQueue: Promise<void> = Promise.resolve();
let snapshotPromise: Promise<void> | null = null;

function targetFingerprint(target: TerminalInputFocusTarget | null) {
  return target ? JSON.stringify(target) : null;
}

function currentTarget() {
  void registryRevision.value;
  if (focusPending.value || desiredTargetId.value !== acknowledgedTargetId.value) return null;
  const id = acknowledgedTargetId.value;
  const registration = acknowledgedRegistration;
  if (!id || !registration || targets.get(id) !== registration) return null;
  const target = registration.focusTarget();
  if (!target || targetFingerprint(target) !== acknowledgedTargetFingerprint) return null;
  return registration;
}

async function ensureFocusEpoch() {
  if (focusEpoch !== null) return;
  if (!snapshotPromise) {
    snapshotPromise = fetchTerminalInputFocusSnapshot()
      .then((snapshot) => {
        focusEpoch = snapshot.focusEpoch;
      })
      .finally(() => {
        snapshotPromise = null;
      });
  }
  await snapshotPromise;
}

function applyAcknowledgedFocus(
  targetId: string | null,
  response: TerminalInputFocusChangeResponse,
  registration: FocusedTerminalInputTarget | null,
) {
  for (const [id, target] of targets) {
    target.applyFocusLease(id === targetId && target === registration ? response.lease : null);
  }
  acknowledgedTargetId.value = targetId;
  acknowledgedRegistration = registration;
  acknowledgedTargetFingerprint = targetFingerprint(response.target);
  registryRevision.value += 1;
}

async function changeFocusOnce(target: TerminalInputFocusTarget | null) {
  await ensureFocusEpoch();
  try {
    return await changeTerminalInputFocus({
      expectedFocusEpoch: focusEpoch as string,
      target,
    });
  } catch (error) {
    if (parseCoreApiError(error)?.code !== "ssh_terminal.stale_focus") throw error;
    const snapshot = await fetchTerminalInputFocusSnapshot();
    focusEpoch = snapshot.focusEpoch;
    return changeTerminalInputFocus({
      expectedFocusEpoch: focusEpoch,
      target,
    });
  }
}

export const focusedTerminalLabel = computed(() => currentTarget()?.label() ?? null);
export const canRunInFocusedTerminal = computed(
  () => currentTarget()?.canAcceptInput() ?? false,
);

export interface TerminalInputTicket {
  readonly label: string;
  readonly target: TerminalInputFocusTarget;
  suspend(): Promise<boolean>;
  send(value: string): Promise<FocusedTerminalRunResult>;
  perform<T>(action: () => Promise<T>): Promise<T>;
  cancel(): Promise<void>;
  valid(): boolean;
}

/**
  * Asynchronous interactions such as paste and history capture the original Pane. A confirmation dialog releases the lease, then may
  * reacquire that target only when no other focus intent occurred and the session and attachment remain unchanged.
 */
export function captureTerminalInput(id: string): TerminalInputTicket | null {
  const candidate = currentTarget();
  const originalTarget = candidate?.focusTarget();
  if (!candidate || candidate.id !== id || !originalTarget || !(candidate.canAcceptRawInput?.() ?? candidate.canAcceptInput())) return null;
  const original: FocusedTerminalInputTarget = candidate;
  const fingerprint = JSON.stringify(originalTarget);
  let expectedIntent = focusIntentSequence;
  let suspended = false;
  let consumed = false;
  const valid = () => !consumed
    && targets.get(id) === original
    && focusIntentSequence === expectedIntent
    && JSON.stringify(original.focusTarget()) === fingerprint
    && (suspended || (currentTarget() === original && (original.canAcceptRawInput?.() ?? original.canAcceptInput())));
  async function resume() {
    if (!valid()) return false;
    if (!suspended) return true;
    const promise = focusTerminalInputTarget(id);
    expectedIntent = focusIntentSequence;
    if (!await promise || !valid()) return false;
    suspended = false;
    return currentTarget() === original && (original.canAcceptRawInput?.() ?? original.canAcceptInput());
  }
  return {
    label: original.label(),
    target: originalTarget,
    valid,
    async suspend() {
      if (!valid()) return false;
      if (suspended) return true;
      suspended = true;
      const promise = clearTerminalInputFocus();
      expectedIntent = focusIntentSequence;
      return await promise && valid();
    },
    async send(value) {
      if (!await resume() || !valid()) { consumed = true; return "unavailable"; }
      consumed = true;
      try { await original.send(value); return "sent"; }
      catch {
        original.applyFocusLease(null);
        return "failed";
      }
    },
    async perform<T>(action: () => Promise<T>) {
      if (!await resume() || !valid()) { consumed = true; throw new Error("Terminal input target changed"); }
      consumed = true;
      try { return await action(); }
      catch (error) { original.applyFocusLease(null); throw error; }
    },
    async cancel() {
      if (suspended && valid()) await resume();
      consumed = true;
    },
  };
}

export function isTerminalInputTargetFocused(id: string, targetFocusEpoch: string | null) {
  return currentTarget()?.id === id
    && focusEpoch !== null
    && focusEpoch === targetFocusEpoch;
}

export function registerTerminalInputTarget(target: FocusedTerminalInputTarget) {
  const replaced = targets.get(target.id);
  if (replaced && replaced !== target) replaced.applyFocusLease(null);
  targets.set(target.id, target);
  registryRevision.value += 1;
  return () => {
    if (targets.get(target.id) !== target) return;
    targets.delete(target.id);
    target.applyFocusLease(null);
    if (desiredTargetId.value === target.id || acknowledgedTargetId.value === target.id) {
      void focusTerminalInputTarget(null);
    }
    registryRevision.value += 1;
  };
}

/// Serializes renderer focus intents against the process-wide Core broker.
/// Input remains disabled until the latest intent is acknowledged. A stale ack
/// may advance Core's epoch but can never overwrite a newer A→B→A intent.
export function focusTerminalInputTarget(id: string | null): Promise<boolean> {
  const normalizedId = id !== null && targets.has(id) ? id : null;
  const existingRegistration = normalizedId ? targets.get(normalizedId) ?? null : null;
  const existingTarget = existingRegistration?.focusTarget() ?? null;
  if (
    normalizedId !== null
    && !focusPending.value
    && desiredTargetId.value === normalizedId
    && acknowledgedTargetId.value === normalizedId
    && acknowledgedRegistration === existingRegistration
    && acknowledgedTargetFingerprint === targetFingerprint(existingTarget)
    && existingTarget !== null
    && (existingRegistration?.canAcceptRawInput?.() ?? existingRegistration?.canAcceptInput() ?? false)
  ) {
    return Promise.resolve(true);
  }
  desiredTargetId.value = normalizedId;
  focusPending.value = true;
  const intentSequence = ++focusIntentSequence;
  const run = async () => {
    const registration = normalizedId ? targets.get(normalizedId) ?? null : null;
    const target = registration?.focusTarget() ?? null;
    const effectiveTargetId = target ? normalizedId : null;
    try {
      const response = await changeFocusOnce(target);
      focusEpoch = response.focusEpoch;
      if (intentSequence === focusIntentSequence) {
        applyAcknowledgedFocus(effectiveTargetId, response, effectiveTargetId ? registration : null);
        desiredTargetId.value = effectiveTargetId;
        focusPending.value = false;
        return true;
      }
      return false;
    } catch {
      if (intentSequence === focusIntentSequence) {
        for (const registered of targets.values()) registered.applyFocusLease(null);
        acknowledgedTargetId.value = null;
        acknowledgedRegistration = null;
        acknowledgedTargetFingerprint = null;
        focusPending.value = false;
        registryRevision.value += 1;
      }
      return false;
    }
  };
  const result = focusQueue.then(run, run);
  focusQueue = result.then(() => undefined, () => undefined);
  return result;
}

export function clearTerminalInputFocus() {
  return focusTerminalInputTarget(null);
}

export function resetTerminalInputFocusForTests() {
  targets.clear();
  desiredTargetId.value = null;
  acknowledgedTargetId.value = null;
  focusPending.value = false;
  registryRevision.value += 1;
  focusEpoch = null;
  acknowledgedRegistration = null;
  acknowledgedTargetFingerprint = null;
  focusIntentSequence = 0;
  focusQueue = Promise.resolve();
  snapshotPromise = null;
}

export async function runInFocusedTerminal(command: string): Promise<FocusedTerminalRunResult> {
  const target = currentTarget();
  if (!target?.canAcceptInput()) return "unavailable";
  try {
    // Input is intentionally not retried. An IPC failure may be response loss
    // after Core accepted the bytes, so the user must inspect the terminal
    // before deciding whether to run the command again.
    await target.send(`${command}\r`);
    return "sent";
  } catch {
    return "failed";
  }
}
