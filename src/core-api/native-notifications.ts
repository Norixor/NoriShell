import { invoke } from "@tauri-apps/api/core";

import type { AppLocale } from "../locales";
import type {
  ForwardSessionId,
  SshSessionId,
  TelnetSessionId,
  TransferId,
  WireSequence,
} from "./generated/core-api";

export type NativeNotificationPermission = "notDetermined" | "granted" | "denied" | "unavailable";
export type NativeNotificationDelivery = "idle" | "accepted" | "permissionDenied" | "unavailable" | "failed" | "unknown" | "rateLimited" | "suppressed";
export type NativeNotificationFailure = "windowNotAllowed" | "serviceStopped" | "busy" | "permissionDenied" | "unavailable" | "dispatchFailed" | "timeout" | "rateLimited";

export interface NativeNotificationPermissionSnapshot {
  permission: NativeNotificationPermission;
  lastDelivery: NativeNotificationDelivery;
}

// Native notification clicks contain only opaque resource fences. Consumers
// must revalidate the exact generation/revision before focusing an existing
// resource and must never connect or reconnect from this event.
export type NativeResourceNotificationClick =
  | { kind: "sftpTransfer"; eventId: string; transferId: TransferId; stateRevision: WireSequence }
  | { kind: "ssh"; eventId: string; sessionId: SshSessionId; generation: WireSequence; stateRevision: WireSequence }
  | { kind: "telnet"; eventId: string; sessionId: TelnetSessionId; generation: WireSequence; stateRevision: WireSequence }
  | { kind: "desktop"; eventId: string; sessionId: string; generation: WireSequence; revision: WireSequence }
  | { kind: "forward"; eventId: string; sessionId: ForwardSessionId; generation: WireSequence; stateRevision: WireSequence };

export const nativeResourceNotificationClickEvent = "native-resource-notification-click";

const meta = () => ({ requestId: crypto.randomUUID() });

export function getNativeNotificationPermission(): Promise<NativeNotificationPermissionSnapshot> {
  return invoke("native_notification_permission_get", { request: { meta: meta() } });
}

// Only call from an explicit permission action, never from component mount/watch.
export function requestNativeNotificationPermission(): Promise<NativeNotificationPermissionSnapshot> {
  return invoke("native_notification_permission_request", { request: { meta: meta() } });
}

export function testNativeNotification(locale: AppLocale): Promise<NativeNotificationPermissionSnapshot> {
  return invoke("native_notification_test", { request: { meta: meta(), locale } });
}

export function setNativeNotificationContext(locale: string): Promise<void> {
  return invoke("native_notification_context_set", { request: { meta: meta(), locale: locale === "en" ? "en" : "zh-CN" } });
}

export function nativeNotificationFailure(error: unknown): NativeNotificationFailure {
  const code = typeof error === "object" && error !== null && "code" in error ? error.code : null;
  switch (code) {
    case "windowNotAllowed":
    case "serviceStopped":
    case "busy":
    case "permissionDenied":
    case "unavailable":
    case "dispatchFailed":
    case "timeout":
    case "rateLimited":
      return code;
    default:
      return "unavailable";
  }
}
