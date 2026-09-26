import { invoke } from "@tauri-apps/api/core";
import type { DesktopAvailability, DesktopPasswordStage, DesktopProfile, DesktopSessionSummary, DesktopInputRequest, DesktopPrompt, DesktopPromptDecision, WireSequence } from "./generated/core-api";
const meta = () => ({ requestId: crypto.randomUUID() });
const sessionRequest = (session: DesktopSessionSummary) => ({ meta: meta(), sessionId: session.id, generation: session.generation });
export const desktopClient = {
  availability: () => invoke<DesktopAvailability[]>("desktop_availability"),
  profiles: () => invoke<DesktopProfile[]>("desktop_profile_list", { meta: meta() }),
  save: (profile: DesktopProfile, passwordStage: DesktopPasswordStage | null = null) => invoke<DesktopProfile>("desktop_profile_save", { request: { meta: meta(), profile, passwordStage } }),
  delete: (profile: DesktopProfile) => invoke<void>("desktop_profile_delete", { request: { meta: meta(), id: profile.id, expectedRevision: profile.revision } }),
  open: (profile: DesktopProfile) => invoke<DesktopSessionSummary>("desktop_session_open", { request: { meta: meta(), operationId: crypto.randomUUID(), profile } }),
  snapshot: () => invoke<DesktopSessionSummary[]>("desktop_session_snapshot"),
  disconnect: (session: DesktopSessionSummary) => invoke<void>("desktop_session_disconnect", { request: sessionRequest(session) }),
  close: (session: DesktopSessionSummary) => invoke<void>("desktop_session_close", { request: sessionRequest(session) }),
  focus: (session: DesktopSessionSummary | null) => invoke<WireSequence>("desktop_focus_change", { request: { meta: meta(), sessionId: session?.id ?? null, generation: session?.generation ?? null } }),
  frame: (session: DesktopSessionSummary, afterSequence: WireSequence) => invoke<ArrayBuffer>("desktop_frame_get", { request: { ...sessionRequest(session), afterSequence } }),
  resolution: (session: DesktopSessionSummary, width: number, height: number) => invoke<void>("desktop_resolution_set", { request: { ...sessionRequest(session), width, height } }),
  input: (request: Omit<DesktopInputRequest, "meta">) => invoke<void>("desktop_input", { request: { meta: meta(), ...request } }),
  mute: (session: DesktopSessionSummary, muted: boolean) => invoke<void>("desktop_audio_mute", { request: { ...sessionRequest(session), muted } }),
  clipboard: (session: DesktopSessionSummary) => invoke<string | null>("desktop_clipboard_get", { request: sessionRequest(session) }),
  prompt: (id: string) => invoke<DesktopPrompt>("desktop_prompt_get", { id }),
  decide: (decision: DesktopPromptDecision) => invoke<void>("desktop_prompt_decide", { decision }),
};
