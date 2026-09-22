import { invoke } from "@tauri-apps/api/core";
import { createUuidV7 } from "./ids";
import type { SshHostKeyChallenge, SshKeyboardInteractiveChallenge, MetricsHostKeyChallenge, MetricsKeyboardInteractiveChallenge, SshHostKeyDecisionRequest, SshKeyboardInteractiveResponseRequest, MetricsHostKeyDecisionRequest, MetricsKeyboardInteractiveRespondRequest } from "./generated/core-api";
type OperationFields = "meta" | "operationId" | "idempotencyKey";
export type SecureChallengeInput =
  | { kind: "sshHostKey"; request: Omit<SshHostKeyDecisionRequest, OperationFields | "decision"> }
  | { kind: "sshKeyboard"; request: Omit<SshKeyboardInteractiveResponseRequest, "meta" | "answers"> }
  | { kind: "metricsHostKey"; request: Omit<MetricsHostKeyDecisionRequest, OperationFields | "decision"> }
  | { kind: "metricsKeyboard"; request: Omit<MetricsKeyboardInteractiveRespondRequest, OperationFields | "answerRefIds"> };
export interface SecureChallengePrompt {
  id: string;
  content:
    | { kind: "sshHostKey"; challenge: SshHostKeyChallenge }
    | { kind: "sshKeyboard"; challenge: SshKeyboardInteractiveChallenge }
    | { kind: "metricsHostKey"; challenge: MetricsHostKeyChallenge }
    | { kind: "metricsKeyboard"; challenge: MetricsKeyboardInteractiveChallenge };
}
export function requestSecureSshChallenge(input: SecureChallengeInput): Promise<boolean> {
  const operationId = createUuidV7();
  const request = { ...input.request, meta: { requestId: crypto.randomUUID() }, operationId, idempotencyKey: `secure-ssh-${operationId}`,
    ...(input.kind === "sshHostKey" ? { decision: "reject" } : input.kind === "metricsHostKey" ? { decision: "reject" } : input.kind === "sshKeyboard" ? { answers: [] } : { answerRefIds: [] }),
  };
  return invoke<boolean>("secure_ssh_challenge_open", { target: { kind: input.kind, request } });
}
export const secureSshChallengeClient = {
  get: (id: string) => invoke<SecureChallengePrompt>("secure_ssh_challenge_get", { id }),
  submit: (id: string, approved: boolean, answers: string[]) => invoke<void>("secure_ssh_challenge_submit", { id, approved, answers }),
  cancel: (id: string) => invoke<void>("secure_ssh_challenge_cancel", { id }),
};
