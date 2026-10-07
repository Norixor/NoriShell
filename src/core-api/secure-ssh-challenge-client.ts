import { invoke } from "@tauri-apps/api/core";
import { secureInvoke } from "../secure-transport";
import { createUuidV7 } from "./ids";
import type { SshHostKeyChallenge, SshKeyboardInteractiveChallenge, MetricsKeyboardInteractiveChallenge, SshHostKeyDecisionRequest, SshKeyboardInteractiveResponseRequest, MetricsKeyboardInteractiveRespondRequest } from "./generated/core-api";
type OperationFields = "meta" | "operationId" | "idempotencyKey";
export type SecureChallengeInput =
  | { kind: "sshHostKey"; request: Omit<SshHostKeyDecisionRequest, OperationFields | "decision"> }
  | { kind: "sshKeyboard"; request: Omit<SshKeyboardInteractiveResponseRequest, "meta" | "answers"> }
  | { kind: "metricsKeyboard"; request: Omit<MetricsKeyboardInteractiveRespondRequest, OperationFields | "answerRefIds"> };
export interface SecureChallengePrompt {
  id: string;
  content:
    | { kind: "sshHostKey"; challenge: SshHostKeyChallenge }
    | { kind: "sftpHostKey"; challenge: { endpoint: string; algorithm: string; fingerprintSha256: string } }
    | { kind: "sshKeyboard"; challenge: SshKeyboardInteractiveChallenge }
    | { kind: "metricsKeyboard"; challenge: MetricsKeyboardInteractiveChallenge };
}
export function requestSecureSshChallenge(input: SecureChallengeInput): Promise<boolean> {
  const operationId = createUuidV7();
  const request = { ...input.request, meta: { requestId: crypto.randomUUID() }, operationId, idempotencyKey: `secure-ssh-${operationId}`,
    ...(input.kind === "sshHostKey" ? { decision: "reject" } : input.kind === "sshKeyboard" ? { answers: [] } : { answerRefIds: [] }),
  };
  return invoke<boolean>("secure_ssh_challenge_open", { target: { kind: input.kind, request } });
}
export const secureSshChallengeClient = {
  get: (id: string) => secureInvoke<SecureChallengePrompt>("secure_ssh_challenge_get", { id }),
  submit: (id: string, approved: boolean, answers: string[]) => secureInvoke<void>("secure_ssh_challenge_submit", { id, approved, answers }),
  cancel: (id: string) => secureInvoke<void>("secure_ssh_challenge_cancel", { id }),
};
