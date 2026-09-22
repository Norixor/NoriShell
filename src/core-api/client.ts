import { Channel, invoke, isTauri } from "@tauri-apps/api/core";

import {
  coreApiCommands,
  type CoreApiError,
  type ExitReadiness,
  type HostCatalogEntry,
  type HostCatalogSort,
  type HostGroupSummary,
  type HostConnectionConfigSummary,
  type HostOrganizationSummary,
  type HostSummary,
  type HostConfiguredCreateRequest,
  type HostConfiguredCreateResponse,
  type HostCreatePasswordStageRequest,
  type HostCreatePasswordStageResponse,
  type HostCreatePasswordCancelRequest,
  type HostCreatePasswordCancelResponse,
  type HostTagSummary,
  type RoutePlanReplaceRequest,
  type RoutePlanSummary,
  type AuthenticationPlanReplaceRequest,
  type AuthenticationPlanSummary,
  type AlgorithmPolicyReplaceRequest,
  type AlgorithmPolicyCatalog,
  type AlgorithmPolicySummary,
  type HeartbeatPolicyReplaceRequest,
  type HeartbeatPolicySummary,
  type MonitoringPolicyReplaceRequest,
  type MonitoringPolicyReplaceResponse,
  type InstalledPluginSummary,
  type PluginAuditEntry,
  type PluginAuditListRequest,
  type PluginCapabilityGrant,
  type PluginCapabilityGrantsReplaceRequest,
  type PluginContributionCopyRequest,
  type PluginContributionCopyResponse,
  type PluginContributionInvokeRequest,
  type PluginContributionPanel,
  type PluginHostScopeListRequest,
  type PluginHostScopeReplaceRequest,
  type PluginHostScopeSnapshot,
  type PluginHostApprovalDecisionRequest,
  type PluginHostApprovalDecisionResponse,
  type PluginHostApprovalGetRequest,
  type PluginHostApprovalSummary,
  type PluginRemoteApprovalPrompt,
  type PluginRemoteApprovalGetRequest,
  type PluginRemoteApprovalDecisionRequest,
  type PluginCredentialInputDecisionRequest,
  type PluginIsolatedBridgeRequest,
  type PluginIsolatedBridgeResponse,
  type PluginIsolatedSurfaceContent,
  type PluginIsolatedSurfaceContentRequest,
  type PluginSpecialPermissionDecisionRequest,
  type PluginSpecialPermissionDecisionResponse,
  type PluginSpecialPermissionGetRequest,
  type PluginSpecialPermissionOpenRequest,
  type PluginSpecialPermissionSnapshot,
  type PluginExtensionTargetContext,
  type PluginExtensionTargetDefinition,
  type PluginTargetContextCloseRequest,
  type PluginTargetContextOpenRequest,
  type PluginUiActionRequest,
  type PluginUiActionResponse,
  type PluginUiContribution,
  type PluginUiContributionListRequest,
  type PluginSshSyncBrowserReadRequest,
  type PluginSshSyncBrowserSnapshot,
  type SshSyncSecureDecisionRequest,
  type SshSyncSecureDecisionResponse,
  type SshSyncSecurePrompt,
  type PluginLocalInstallRequest,
  type PluginNavigationItem,
  type PluginLocalPackageCancelRequest,
  type PluginLocalPackagePreview,
  type PluginOperationRequest,
  type PluginOperationPermissionList,
  type PluginOperationPermissionListRequest,
  type PluginOperationPermissionRevokeRequest,
  type PluginOperationPermissionsClearRequest,
  type PluginOperationSummary,
  type PluginReadiness,
  type PluginReadinessGetRequest,
  type PluginSafeModeNextStartRequest,
  type PluginStateChangeRequest,
  type PluginSettingsGetRequest,
  type PluginSettingsReplaceRequest,
  type PluginSettingsResetRequest,
  type PluginSettingsSnapshot,
  type PluginLocale,
  type PluginLocaleSetRequest,
  type PluginTerminalInputDecisionRequest,
  type PluginTerminalInputDecisionResponse,
  type PluginTerminalInputGetRequest,
  type PluginTerminalInputOpenRequest,
  type PluginTerminalInputProposal,
  type PluginTerminalObserveAttachRequest,
  type PluginTerminalObserveAttachResponse,
  type PluginTerminalObserveDetachRequest,
  type PluginUninstallRequest,
  type MetricsHostKeyDecisionRequest,
  type MetricsKeyboardInteractiveAnswerPrepareRequest,
  type MetricsKeyboardInteractiveAnswerPrepareResponse,
  type MetricsKeyboardInteractiveRespondRequest,
  type MetricsRetryRequest,
  type MetricsSessionSummary,
  type MetricsStopRequest,
  type ServerOverviewSnapshot,
  type LoginAutomationReplaceRequest,
  type LoginAutomationConfirmRequest,
  type LoginAutomationSecretCreateRequest,
  type LoginAutomationSecretCreateResponse,
  type LoginAutomationSecretCancelRequest,
  type LoginAutomationSecretCancelResponse,
  type LoginAutomationSummary,
  type IdentitySummary,
  type IdentityDeleteImpact,
  type IdentityDeleteResponse,
  type CredentialRefSummary,
  type CredentialRefListRequest,
  type CredentialKind,
  type CredentialImportRequest,
  type PrivateKeyFileImportRequest,
  type TransientCredentialPrepareRequest,
  type TransientCredentialRef,
  type KnownHostSummary,
  type KnownHostDeleteResponse,
  type LocalSessionAttachRequest,
  type LocalSessionAttachResponse,
  type LocalSessionAttachment,
  type LocalSessionAttachmentHeartbeatRequest,
  type LocalSessionDetails,
  type LocalSessionDetachRequest,
  type LocalSessionDetachResult,
  type LocalSessionEvent,
  type LocalSessionInputLease,
  type LocalSessionInputLeaseRenewRequest,
  type LocalSessionInputRequest,
  type LocalSessionOpenRequest,
  type LocalSessionOpenResponse,
  type LocalSessionResizeRequest,
  type LocalSessionSnapshot,
  type LocalSessionSummary,
  type OpenSshConfigCommitResponse,
  type OpenSshConfigPreviewResponse,
  type RecentConnectionSummary,
  type LocalSessionTerminateRequest,
  type SshHostKeyDecisionRequest,
  type KeyboardInteractiveCredentialCreateRequest,
  type SshKeyboardInteractiveAnswerPrepareRequest,
  type SshKeyboardInteractiveAnswerPrepareResponse,
  type SshKeyboardInteractiveResponseRequest,
  type SshAgentCredentialCreateRequest,
  type AgentIdentityKind,
  type SshAgentKeySummary,
  type SshSessionAttachRequest,
  type SshSessionAttachResponse,
  type SshSessionAttachment,
  type SshSessionAttachmentHeartbeatRequest,
  type SshSessionDetails,
  type SshSessionDetachRequest,
  type SshSessionDetachResult,
  type SshSessionDisconnectRequest,
  type SshSessionEvent,
  type SshSessionInputLease,
  type SshSessionInputLeaseRenewRequest,
  type SshSessionInputRequest,
  type SshConnectionTestRequest,
  type SshConnectionTestResponse,
  type SshSessionOpenRequest,
  type SshSessionOpenResponse,
  type SshSessionReconnectRequest,
  type SshLoginAutomationTakeoverRequest,
  type SshLoginAutomationTakeoverResponse,
  type SshSessionResizeRequest,
  type SshSessionSnapshot,
  type SshTerminalInputFocusChangeRequest,
  type SshTerminalInputFocusChangeResponse,
  type SshTerminalInputFocusSnapshot,
  type TerminalInputFocusChangeRequest,
  type TerminalInputFocusChangeResponse,
  type TerminalInputFocusSnapshot,
  type TelnetInputLease,
  type TelnetSessionAttachRequest,
  type TelnetSessionAttachResponse,
  type TelnetSessionAttachment,
  type TelnetSessionAttachmentHeartbeatRequest,
  type TelnetSessionDetachRequest,
  type TelnetSessionDetachResponse,
  type TelnetSessionDisconnectRequest,
  type TelnetSessionEvent,
  type TelnetSessionInputLeaseRenewRequest,
  type TelnetSessionInputRequest,
  type TelnetSessionOpenRequest,
  type TelnetSessionOpenResponse,
  type TelnetSessionReconnectRequest,
  type TelnetSessionResizeRequest,
  type TelnetSessionSnapshot,
  type TelnetSessionSummary,
  type ForwardCleanupRetainRequest,
  type ForwardRuleCreateRequest,
  type ForwardRuleDeleteRequest,
  type ForwardRuleListResponse,
  type ForwardRulePreflightRequest,
  type ForwardRulePreflightResponse,
  type ForwardRuleSummary,
  type ForwardRuleUpdateRequest,
  type ForwardSessionEvent,
  type ForwardSessionSnapshot,
  type ForwardSessionStartRequest,
  type ForwardSessionStopRequest,
  type ForwardSessionSummary,
  type SftpDirectoryListRequest,
  type SftpDirectoryListCancelRequest,
  type SftpDirectoryListing,
  type SftpFilePreview,
  type SftpFilePreviewRequest,
  type SftpFileTailRequest,
  type SftpFileTailResult,
  type SftpFileMutationRequest,
  type SftpFileMutationResult,
  type SftpLocalBoundary,
  type SftpLocalBoundaryKind,
  type SftpLocalDirectoryCapability,
  type SftpLocalDirectoryCreateChildRequest,
  type SftpLocalDirectoryListRequest,
  type SftpLocalDirectoryListing,
  type SftpLocalDirectoryOpenChildRequest,
  type SftpLocalDirectoryReleaseRequest,
  type SftpSessionDisconnectRequest,
  type SftpSessionOpenRequest,
  type SftpSessionSnapshot,
  type SftpSessionSummary,
  type SftpRemoteCleanupRetryRequest,
  type SftpRemoteCleanupRetainRequest,
  type SftpTransferActionRequest,
  type SftpTransferEnqueueRequest,
  type SftpTransferIntentActionRequest,
  type SftpTransferIntentCleanupRetryRequest,
  type SftpTransferIntentCleanupRetainRequest,
  type SftpTransferIntentEnqueueRequest,
  type SftpTransferIntentPrepareRequest,
  type SftpTransferIntentPrepared,
  type SftpTransferIntentSnapshot,
  type SftpTransferIntentSummary,
  type SftpTransferSummary,
  type TerminalWorkspaceLayout,
  type TerminalWorkspaceLayoutSnapshot,
  type VaultStatus,
} from "./generated/core-api";
import { createUuidV7 } from "./ids";

function requestMeta() {
  return { requestId: crypto.randomUUID() };
}

export function canUseDesktopCore(): boolean {
  return isTauri();
}

export async function fetchVaultStatus(): Promise<VaultStatus> {
  return invoke(coreApiCommands.vaultStatus, {
    request: { meta: requestMeta() },
  });
}

export async function createVault(
  password: string,
  passwordConfirmation: string,
): Promise<VaultStatus> {
  return invoke(coreApiCommands.vaultCreate, {
    request: {
      meta: requestMeta(),
      password,
      passwordConfirmation,
    },
  });
}

export async function unlockVault(password: string): Promise<VaultStatus> {
  return invoke(coreApiCommands.vaultUnlock, {
    request: { meta: requestMeta(), password },
  });
}

export async function lockVault(): Promise<VaultStatus> {
  return invoke(coreApiCommands.vaultLock, {
    request: { meta: requestMeta() },
  });
}

export async function enableVaultAutoUnlock(password: string): Promise<VaultStatus> {
  return invoke(coreApiCommands.vaultAutoUnlockEnable, {
    request: { meta: requestMeta(), password },
  });
}

export async function disableVaultAutoUnlock(): Promise<VaultStatus> {
  return invoke(coreApiCommands.vaultAutoUnlockDisable, {
    request: { meta: requestMeta() },
  });
}

export async function listInstalledPlugins(): Promise<InstalledPluginSummary[]> {
  return invoke(coreApiCommands.pluginInstalledList, {
    request: { meta: requestMeta() },
  });
}

export async function getSshSyncSecurePrompt(promptId: string): Promise<SshSyncSecurePrompt> {
  return invoke("ssh_sync_secure_prompt_get", {
    request: { meta: requestMeta(), promptId },
  });
}

export async function decideSshSyncSecurePrompt(
  input: Omit<SshSyncSecureDecisionRequest, "meta">,
): Promise<SshSyncSecureDecisionResponse> {
  return invoke("ssh_sync_secure_prompt_decide", {
    request: { meta: requestMeta(), ...input },
  });
}

export async function listPluginContributions(): Promise<PluginContributionPanel[]> {
  return invoke(coreApiCommands.pluginContributionList, {
    request: { meta: requestMeta() },
  });
}

export async function invokePluginContribution(
  input: Omit<PluginContributionInvokeRequest, "meta">,
): Promise<PluginContributionPanel> {
  return invoke(coreApiCommands.pluginContributionInvoke, {
    request: { meta: requestMeta(), ...input } satisfies PluginContributionInvokeRequest,
  });
}

export async function preparePluginContributionCopy(
  input: Omit<PluginContributionCopyRequest, "meta">,
): Promise<PluginContributionCopyResponse> {
  return invoke(coreApiCommands.pluginContributionCopy, {
    request: { meta: requestMeta(), ...input } satisfies PluginContributionCopyRequest,
  });
}

export async function listPluginExtensionTargets(): Promise<PluginExtensionTargetDefinition[]> {
  return invoke(coreApiCommands.pluginExtensionTargetList, {
    request: { meta: requestMeta() },
  });
}

export async function openPluginTargetContext(
  input: Omit<PluginTargetContextOpenRequest, "meta">,
): Promise<PluginExtensionTargetContext> {
  return invoke(coreApiCommands.pluginTargetContextOpen, {
    request: { meta: requestMeta(), ...input } satisfies PluginTargetContextOpenRequest,
  });
}

export async function closePluginTargetContext(
  input: Omit<PluginTargetContextCloseRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.pluginTargetContextClose, {
    request: { meta: requestMeta(), ...input } satisfies PluginTargetContextCloseRequest,
  });
}

export async function listPluginUiContributions(
  input: Omit<PluginUiContributionListRequest, "meta">,
): Promise<PluginUiContribution[]> {
  return invoke(coreApiCommands.pluginUiContributionList, {
    request: { meta: requestMeta(), ...input } satisfies PluginUiContributionListRequest,
  });
}

export async function invokePluginUiAction(
  input: Omit<PluginUiActionRequest, "meta">,
): Promise<PluginUiActionResponse> {
  return invoke(coreApiCommands.pluginUiAction, {
    request: { meta: requestMeta(), ...input } satisfies PluginUiActionRequest,
  });
}

export async function readPluginSshSyncBrowser(
  input: Omit<PluginSshSyncBrowserReadRequest, "meta">,
): Promise<PluginSshSyncBrowserSnapshot> {
  return invoke(coreApiCommands.pluginSshSyncBrowserRead, {
    request: { meta: requestMeta(), ...input } satisfies PluginSshSyncBrowserReadRequest,
  });
}

export async function listPluginNavigation(): Promise<PluginNavigationItem[]> {
  return invoke(coreApiCommands.pluginNavigationList, {
    request: { meta: requestMeta() },
  });
}

export async function listPluginHostScopes(pluginId: string): Promise<PluginHostScopeSnapshot> {
  return invoke(coreApiCommands.pluginHostScopeList, {
    request: { meta: requestMeta(), pluginId } satisfies PluginHostScopeListRequest,
  });
}

export async function replacePluginHostScopes(
  input: Omit<PluginHostScopeReplaceRequest, "meta">,
): Promise<PluginHostScopeSnapshot> {
  return invoke(coreApiCommands.pluginHostScopeReplace, {
    request: { meta: requestMeta(), ...input } satisfies PluginHostScopeReplaceRequest,
  });
}

export async function openPluginHostApproval(approvalId: string): Promise<void> {
  return invoke(coreApiCommands.pluginHostApprovalOpen, {
    request: { meta: requestMeta(), approvalId } satisfies PluginHostApprovalGetRequest,
  });
}

export async function getPluginHostApproval(
  approvalId: string,
): Promise<PluginHostApprovalSummary> {
  return invoke(coreApiCommands.pluginHostApprovalGet, {
    request: { meta: requestMeta(), approvalId } satisfies PluginHostApprovalGetRequest,
  });
}

export async function getPluginRemoteApproval(approvalId: string): Promise<PluginRemoteApprovalPrompt> {
  return invoke(coreApiCommands.pluginRemoteApprovalGet, {
    request: { meta: requestMeta(), approvalId } satisfies PluginRemoteApprovalGetRequest,
  });
}

export async function decidePluginRemoteApproval(input: Omit<PluginRemoteApprovalDecisionRequest, "meta">): Promise<void> {
  return invoke(coreApiCommands.pluginRemoteApprovalDecide, {
    request: { meta: requestMeta(), ...input } satisfies PluginRemoteApprovalDecisionRequest,
  });
}

export async function submitPluginCredentialInput(input: Omit<PluginCredentialInputDecisionRequest, "meta">): Promise<void> {
  return invoke(coreApiCommands.pluginCredentialInputSubmit, {
    request: { meta: requestMeta(), ...input } satisfies PluginCredentialInputDecisionRequest,
  });
}

export async function decidePluginHostApproval(
  input: Omit<PluginHostApprovalDecisionRequest, "meta">,
): Promise<PluginHostApprovalDecisionResponse> {
  return invoke(coreApiCommands.pluginHostApprovalDecide, {
    request: { meta: requestMeta(), ...input } satisfies PluginHostApprovalDecisionRequest,
  });
}

export async function getPluginIsolatedSurfaceContent(
  surfaceId: string,
  channelNonce: string,
): Promise<PluginIsolatedSurfaceContent> {
  return invoke(coreApiCommands.pluginIsolatedSurfaceContent, {
    request: {
      meta: requestMeta(),
      surfaceId,
      channelNonce,
    } satisfies PluginIsolatedSurfaceContentRequest,
  });
}

export async function invokePluginIsolatedBridge(
  input: Omit<PluginIsolatedBridgeRequest, "meta">,
): Promise<PluginIsolatedBridgeResponse> {
  return invoke(coreApiCommands.pluginIsolatedBridge, {
    request: { meta: requestMeta(), ...input } satisfies PluginIsolatedBridgeRequest,
  });
}

export async function openPluginSpecialPermission(
  input: Omit<PluginSpecialPermissionOpenRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.pluginSpecialPermissionOpen, {
    request: { meta: requestMeta(), ...input } satisfies PluginSpecialPermissionOpenRequest,
  });
}

export async function getPluginSpecialPermission(
  approvalId: string,
): Promise<PluginSpecialPermissionSnapshot> {
  return invoke(coreApiCommands.pluginSpecialPermissionGet, {
    request: { meta: requestMeta(), approvalId } satisfies PluginSpecialPermissionGetRequest,
  });
}

export async function decidePluginSpecialPermission(
  input: Omit<PluginSpecialPermissionDecisionRequest, "meta">,
): Promise<PluginSpecialPermissionDecisionResponse> {
  return invoke(coreApiCommands.pluginSpecialPermissionDecide, {
    request: { meta: requestMeta(), ...input } satisfies PluginSpecialPermissionDecisionRequest,
  });
}

export async function prepareLocalPluginPackage(): Promise<PluginLocalPackagePreview | null> {
  return invoke(coreApiCommands.pluginLocalPackagePrepare, {
    request: { meta: requestMeta() },
  });
}

export async function getPluginReadiness(): Promise<PluginReadiness> {
  return invoke(coreApiCommands.pluginReadinessGet, {
    request: { meta: requestMeta() } satisfies PluginReadinessGetRequest,
  });
}

export async function listPluginAudit(limit = 100): Promise<PluginAuditEntry[]> {
  return invoke(coreApiCommands.pluginAuditList, {
    request: { meta: requestMeta(), limit } satisfies PluginAuditListRequest,
  });
}

export async function setPluginSafeModeNextStart(enabled: boolean): Promise<PluginReadiness> {
  return invoke(coreApiCommands.pluginSafeModeNextStart, {
    request: { meta: requestMeta(), enabled } satisfies PluginSafeModeNextStartRequest,
  });
}

export async function completePluginSafeModeStartup(): Promise<PluginReadiness> {
  return invoke(coreApiCommands.pluginSafeModeStartupComplete, {
    request: { meta: requestMeta() } satisfies PluginReadinessGetRequest,
  });
}

export async function cancelLocalPluginPackage(preparationId: string): Promise<void> {
  return invoke(coreApiCommands.pluginLocalPackageCancel, {
    request: {
      meta: requestMeta(),
      preparationId,
    } satisfies PluginLocalPackageCancelRequest,
  });
}

export async function installLocalPlugin(input: {
  preparationId: string;
  expectedPackageSha256: string;
  expectedStateVersion: string | null;
  capabilityGrants: PluginCapabilityGrant[];
}): Promise<PluginOperationSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.pluginLocalInstall, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `plugin-local-install-${operationId}`,
      ...input,
    } satisfies PluginLocalInstallRequest,
  });
}

export async function replacePluginCapabilityGrants(
  input: Omit<PluginCapabilityGrantsReplaceRequest, "meta">,
): Promise<InstalledPluginSummary> {
  return invoke(coreApiCommands.pluginCapabilityGrantsReplace, {
    request: { meta: requestMeta(), ...input } satisfies PluginCapabilityGrantsReplaceRequest,
  });
}

async function changePluginState(
  command: typeof coreApiCommands.pluginEnable | typeof coreApiCommands.pluginDisable,
  input: Omit<PluginStateChangeRequest, "meta">,
): Promise<InstalledPluginSummary> {
  return invoke(command, {
    request: { meta: requestMeta(), ...input } satisfies PluginStateChangeRequest,
  });
}

export async function enablePlugin(
  input: Omit<PluginStateChangeRequest, "meta">,
): Promise<InstalledPluginSummary> {
  return changePluginState(coreApiCommands.pluginEnable, input);
}

export async function disablePlugin(
  input: Omit<PluginStateChangeRequest, "meta">,
): Promise<InstalledPluginSummary> {
  return changePluginState(coreApiCommands.pluginDisable, input);
}

export async function getPluginSettings(
  input: Omit<PluginSettingsGetRequest, "meta">,
): Promise<PluginSettingsSnapshot | null> {
  return invoke(coreApiCommands.pluginSettingsGet, {
    request: { meta: requestMeta(), ...input } satisfies PluginSettingsGetRequest,
  });
}

export async function replacePluginSettings(
  input: Omit<PluginSettingsReplaceRequest, "meta">,
): Promise<PluginSettingsSnapshot> {
  return invoke(coreApiCommands.pluginSettingsReplace, {
    request: { meta: requestMeta(), ...input } satisfies PluginSettingsReplaceRequest,
  });
}

export async function resetPluginSettings(
  input: Omit<PluginSettingsResetRequest, "meta">,
): Promise<PluginSettingsSnapshot> {
  return invoke(coreApiCommands.pluginSettingsReset, {
    request: { meta: requestMeta(), ...input } satisfies PluginSettingsResetRequest,
  });
}

export async function setPluginLocale(locale: PluginLocale): Promise<void> {
  return invoke(coreApiCommands.pluginLocaleSet, {
    request: { meta: requestMeta(), locale } satisfies PluginLocaleSetRequest,
  });
}

export async function uninstallPlugin(input: {
  pluginId: string;
  expectedStateVersion: string;
  deleteData: boolean;
}): Promise<PluginOperationSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.pluginUninstall, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `plugin-uninstall-${operationId}`,
      ...input,
    } satisfies PluginUninstallRequest,
  });
}

export async function getPluginOperation(
  input: Omit<PluginOperationRequest, "meta">,
): Promise<PluginOperationSummary> {
  return invoke(coreApiCommands.pluginOperationGet, {
    request: { meta: requestMeta(), ...input } satisfies PluginOperationRequest,
  });
}

export async function cancelPluginOperation(
  input: Omit<PluginOperationRequest, "meta">,
): Promise<PluginOperationSummary> {
  return invoke(coreApiCommands.pluginOperationCancel, {
    request: { meta: requestMeta(), ...input } satisfies PluginOperationRequest,
  });
}

export async function listPluginOperationPermissions(
  pluginId: string,
): Promise<PluginOperationPermissionList> {
  return invoke(coreApiCommands.pluginOperationPermissionsList, {
    request: {
      meta: requestMeta(),
      pluginId,
    } satisfies PluginOperationPermissionListRequest,
  });
}

export async function revokePluginOperationPermission(
  input: Omit<PluginOperationPermissionRevokeRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.pluginOperationPermissionRevoke, {
    request: { meta: requestMeta(), ...input } satisfies PluginOperationPermissionRevokeRequest,
  });
}

export async function clearPluginOperationPermissions(
  input: Omit<PluginOperationPermissionsClearRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.pluginOperationPermissionsClear, {
    request: { meta: requestMeta(), ...input } satisfies PluginOperationPermissionsClearRequest,
  });
}

export async function listPendingPluginTerminalInput(): Promise<PluginTerminalInputProposal[]> {
  return invoke(coreApiCommands.pluginTerminalInputPendingList, {
    request: { meta: requestMeta() },
  });
}

export async function openPluginTerminalInput(
  input: Omit<PluginTerminalInputOpenRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.pluginTerminalInputOpen, {
    request: { meta: requestMeta(), ...input } satisfies PluginTerminalInputOpenRequest,
  });
}

export async function getPluginTerminalInput(
  approvalId: string,
): Promise<PluginTerminalInputProposal> {
  return invoke(coreApiCommands.pluginTerminalInputGet, {
    request: {
      meta: requestMeta(),
      approvalId,
    } satisfies PluginTerminalInputGetRequest,
  });
}

export async function decidePluginTerminalInput(
  input: Omit<PluginTerminalInputDecisionRequest, "meta">,
): Promise<PluginTerminalInputDecisionResponse> {
  return invoke(coreApiCommands.pluginTerminalInputDecide, {
    request: { meta: requestMeta(), ...input } satisfies PluginTerminalInputDecisionRequest,
  });
}

export async function attachPluginTerminalObserver(
  input: Omit<PluginTerminalObserveAttachRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<PluginTerminalObserveAttachResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.pluginTerminalObserveAttach, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `plugin-observe-attach-${operationId}`,
      ...input,
    } satisfies PluginTerminalObserveAttachRequest,
  });
}

export async function detachPluginTerminalObserver(
  input: Omit<PluginTerminalObserveDetachRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<void> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.pluginTerminalObserveDetach, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `plugin-observe-detach-${operationId}`,
      ...input,
    } satisfies PluginTerminalObserveDetachRequest,
  });
}

export async function changeVaultPassword(
  currentPassword: string,
  newPassword: string,
  newPasswordConfirmation: string,
): Promise<VaultStatus> {
  return invoke(coreApiCommands.vaultChangePassword, {
    request: {
      meta: requestMeta(),
      currentPassword,
      newPassword,
      newPasswordConfirmation,
    },
  });
}

export async function listHosts(): Promise<HostSummary[]> {
  return invoke(coreApiCommands.hostList, {
    request: { meta: requestMeta() },
  });
}

export async function listHostCatalog(
  sort: HostCatalogSort = "favoriteThenLabel",
): Promise<HostCatalogEntry[]> {
  return invoke(coreApiCommands.hostCatalogList, {
    request: { meta: requestMeta(), sort },
  });
}

export async function listHostGroups(): Promise<HostGroupSummary[]> {
  return invoke(coreApiCommands.hostGroupList, {
    request: { meta: requestMeta() },
  });
}

export async function createHostGroup(label: string): Promise<HostGroupSummary> {
  return invoke(coreApiCommands.hostGroupCreate, {
    request: { meta: requestMeta(), label },
  });
}

export async function updateHostGroup(input: {
  groupId: string;
  expectedStateVersion: string;
  label: string;
}): Promise<HostGroupSummary> {
  return invoke(coreApiCommands.hostGroupUpdate, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function deleteHostGroup(
  groupId: string,
  expectedStateVersion: string,
): Promise<void> {
  return invoke(coreApiCommands.hostGroupDelete, {
    request: { meta: requestMeta(), groupId, expectedStateVersion },
  });
}

export async function listHostTags(): Promise<HostTagSummary[]> {
  return invoke(coreApiCommands.hostTagList, {
    request: { meta: requestMeta() },
  });
}

export async function createHostTag(label: string): Promise<HostTagSummary> {
  return invoke(coreApiCommands.hostTagCreate, {
    request: { meta: requestMeta(), label },
  });
}

export async function updateHostTag(input: {
  tagId: string;
  expectedStateVersion: string;
  label: string;
}): Promise<HostTagSummary> {
  return invoke(coreApiCommands.hostTagUpdate, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function deleteHostTag(
  tagId: string,
  expectedStateVersion: string,
): Promise<void> {
  return invoke(coreApiCommands.hostTagDelete, {
    request: { meta: requestMeta(), tagId, expectedStateVersion },
  });
}

export async function getHostOrganization(hostId: string): Promise<HostOrganizationSummary> {
  return invoke(coreApiCommands.hostOrganizationGet, {
    request: { meta: requestMeta(), hostId },
  });
}

export async function replaceHostOrganization(input: {
  hostId: string;
  expectedHostStateVersion: string;
  groupId: string | null;
  tagIds: string[];
}): Promise<HostOrganizationSummary> {
  return invoke(coreApiCommands.hostOrganizationReplace, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function updateHostFavorite(input: {
  hostId: string;
  expectedStateVersion: string;
  favorite: boolean;
}): Promise<HostSummary> {
  return invoke(coreApiCommands.hostFavoriteUpdate, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function listRecentConnections(
  limit = 20,
): Promise<RecentConnectionSummary[]> {
  return invoke(coreApiCommands.recentConnectionList, {
    request: { meta: requestMeta(), limit },
  });
}

export async function createHost(input: {
  label: string;
  address: string;
  port?: number;
  username?: string | null;
  identityId?: string | null;
  favorite?: boolean;
}): Promise<HostSummary> {
  return invoke(coreApiCommands.hostCreate, {
    request: {
      meta: requestMeta(),
      label: input.label,
      address: input.address,
      port: input.port ?? 22,
      username: input.username ?? null,
      identityId: input.identityId ?? null,
      favorite: input.favorite ?? false,
    },
  });
}

export async function stageHostCreatePassword(
  input: Omit<HostCreatePasswordStageRequest, "meta">,
): Promise<HostCreatePasswordStageResponse> {
  return invoke(coreApiCommands.hostCreatePasswordStage, {
    request: { meta: requestMeta(), ...input } satisfies HostCreatePasswordStageRequest,
  });
}

export async function cancelHostCreatePassword(
  input: Omit<HostCreatePasswordCancelRequest, "meta">,
): Promise<HostCreatePasswordCancelResponse> {
  return invoke(coreApiCommands.hostCreatePasswordCancel, {
    request: { meta: requestMeta(), ...input } satisfies HostCreatePasswordCancelRequest,
  });
}

export async function createConfiguredHost(
  input: Omit<HostConfiguredCreateRequest, "meta">,
): Promise<HostConfiguredCreateResponse> {
  return invoke(coreApiCommands.hostConfiguredCreate, {
    request: { meta: requestMeta(), ...input } satisfies HostConfiguredCreateRequest,
  });
}

export async function updateHost(input: {
  hostId: string;
  expectedStateVersion: string;
  label: string;
  address: string;
  port: number;
  username: string | null;
  identityId: string | null;
  favorite: boolean;
}): Promise<HostSummary> {
  return invoke(coreApiCommands.hostUpdate, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function getHostConnectionConfig(
  hostId: string,
): Promise<HostConnectionConfigSummary> {
  return invoke(coreApiCommands.hostConnectionConfigGet, {
    request: { meta: requestMeta(), hostId },
  });
}

export async function replaceRoutePlan(
  input: Omit<RoutePlanReplaceRequest, "meta">,
): Promise<RoutePlanSummary> {
  return invoke(coreApiCommands.routePlanReplace, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function replaceAuthenticationPlan(
  input: Omit<AuthenticationPlanReplaceRequest, "meta">,
): Promise<AuthenticationPlanSummary> {
  return invoke(coreApiCommands.authenticationPlanReplace, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function replaceAlgorithmPolicy(
  input: Omit<AlgorithmPolicyReplaceRequest, "meta">,
): Promise<AlgorithmPolicySummary> {
  return invoke(coreApiCommands.algorithmPolicyReplace, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function getAlgorithmPolicyCatalog(): Promise<AlgorithmPolicyCatalog> {
  return invoke(coreApiCommands.algorithmPolicyCatalogGet, {
    request: { meta: requestMeta() },
  });
}

export async function replaceHeartbeatPolicy(
  input: Omit<HeartbeatPolicyReplaceRequest, "meta">,
): Promise<HeartbeatPolicySummary> {
  return invoke(coreApiCommands.heartbeatPolicyReplace, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function replaceMonitoringPolicy(
  input: Omit<MonitoringPolicyReplaceRequest, "meta">,
): Promise<MonitoringPolicyReplaceResponse> {
  return invoke(coreApiCommands.monitoringPolicyReplace, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function fetchServerOverview(): Promise<ServerOverviewSnapshot> {
  return invoke(coreApiCommands.serverOverviewSnapshot, {
    request: { meta: requestMeta() },
  });
}

export async function reconcileMetrics(): Promise<MetricsSessionSummary[]> {
  return invoke(coreApiCommands.metricsReconcile, {
    request: { meta: requestMeta() },
  });
}

export async function retryMetrics(
  input: Omit<MetricsRetryRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<MetricsSessionSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.metricsRetry, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `metrics-retry-${operationId}`,
      ...input,
    } satisfies MetricsRetryRequest,
  });
}

export async function stopMetrics(
  input: Omit<MetricsStopRequest, "meta">,
): Promise<MetricsSessionSummary> {
  return invoke(coreApiCommands.metricsStop, {
    request: { meta: requestMeta(), ...input } satisfies MetricsStopRequest,
  });
}

export async function decideMetricsHostKey(
  input: Omit<MetricsHostKeyDecisionRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<MetricsSessionSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.metricsHostKeyDecide, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `metrics-host-key-${operationId}`,
      ...input,
    } satisfies MetricsHostKeyDecisionRequest,
  });
}

export async function prepareMetricsKeyboardInteractiveAnswer(
  input: Omit<MetricsKeyboardInteractiveAnswerPrepareRequest, "meta" | "answerRefId">,
): Promise<MetricsKeyboardInteractiveAnswerPrepareResponse> {
  return invoke(coreApiCommands.metricsKeyboardInteractiveAnswerPrepare, {
    request: {
      meta: requestMeta(),
      answerRefId: createUuidV7(),
      ...input,
    } satisfies MetricsKeyboardInteractiveAnswerPrepareRequest,
  });
}

export async function respondMetricsKeyboardInteractive(
  input: Omit<MetricsKeyboardInteractiveRespondRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<MetricsSessionSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.metricsKeyboardInteractiveRespond, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `metrics-keyboard-interactive-${operationId}`,
      ...input,
    } satisfies MetricsKeyboardInteractiveRespondRequest,
  });
}

export async function replaceLoginAutomation(
  input: Omit<LoginAutomationReplaceRequest, "meta">,
): Promise<LoginAutomationSummary> {
  return invoke(coreApiCommands.loginAutomationReplace, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function confirmLoginAutomation(
  input: Omit<LoginAutomationConfirmRequest, "meta">,
): Promise<LoginAutomationSummary> {
  return invoke(coreApiCommands.loginAutomationConfirm, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function createLoginAutomationSecret(
  input: Omit<LoginAutomationSecretCreateRequest, "meta">,
): Promise<LoginAutomationSecretCreateResponse> {
  return invoke(coreApiCommands.loginAutomationSecretCreate, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function cancelLoginAutomationSecret(
  input: Omit<LoginAutomationSecretCancelRequest, "meta">,
): Promise<LoginAutomationSecretCancelResponse> {
  return invoke(coreApiCommands.loginAutomationSecretCancel, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function deleteHost(
  hostId: string,
  expectedStateVersion: string,
): Promise<void> {
  return invoke(coreApiCommands.hostDelete, {
    request: { meta: requestMeta(), hostId, expectedStateVersion },
  });
}

export async function listIdentities(): Promise<IdentitySummary[]> {
  return invoke(coreApiCommands.identityList, {
    request: { meta: requestMeta() },
  });
}

export async function listCredentialRefs(identityId: string): Promise<CredentialRefSummary[]> {
  const request: CredentialRefListRequest = {
    meta: requestMeta(),
    identityId,
  };
  return invoke(coreApiCommands.credentialRefList, { request });
}

export async function createIdentity(
  label: string,
  username: string | null,
): Promise<IdentitySummary> {
  return invoke(coreApiCommands.identityCreate, {
    request: { meta: requestMeta(), label, username },
  });
}

export async function updateIdentity(input: {
  identityId: string;
  expectedStateVersion: string;
  label: string;
  username: string | null;
}): Promise<IdentitySummary> {
  return invoke(coreApiCommands.identityUpdate, {
    request: { meta: requestMeta(), ...input },
  });
}

export async function fetchIdentityDeleteImpact(
  identityId: string,
): Promise<IdentityDeleteImpact> {
  return invoke(coreApiCommands.identityDeleteImpact, {
    request: { meta: requestMeta(), identityId },
  });
}

export async function deleteIdentity(
  identityId: string,
  expectedStateVersion: string,
): Promise<IdentityDeleteResponse> {
  return invoke(coreApiCommands.identityDelete, {
    request: { meta: requestMeta(), identityId, expectedStateVersion },
  });
}

export async function listSshAgentKeys(): Promise<SshAgentKeySummary[]> {
  return invoke(coreApiCommands.sshAgentKeyList, {
    request: { meta: requestMeta() },
  });
}

export async function createSshAgentCredential(input: {
  operationId?: string;
  idempotencyKey?: string;
  identityId: string;
  keyHandle: string;
  expectedIdentityKind: AgentIdentityKind;
  priority: number;
  label: string;
}): Promise<CredentialRefSummary> {
  const operationId = input.operationId ?? createUuidV7();
  const request: SshAgentCredentialCreateRequest = {
    meta: requestMeta(),
    operationId,
    idempotencyKey: input.idempotencyKey ?? `ssh-agent-credential-${operationId}`,
    identityId: input.identityId,
    keyHandle: input.keyHandle,
    expectedIdentityKind: input.expectedIdentityKind,
    priority: input.priority,
    label: input.label,
  };
  return invoke(coreApiCommands.sshAgentCredentialCreate, { request });
}

export async function createKeyboardInteractiveCredential(input: {
  operationId?: string;
  idempotencyKey?: string;
  identityId: string;
  maxRounds: number;
  priority: number;
  label: string;
}): Promise<CredentialRefSummary> {
  const operationId = input.operationId ?? createUuidV7();
  const request: KeyboardInteractiveCredentialCreateRequest = {
    meta: requestMeta(),
    operationId,
    idempotencyKey: input.idempotencyKey
      ?? `keyboard-interactive-credential-${operationId}`,
    identityId: input.identityId,
    maxRounds: input.maxRounds,
    priority: input.priority,
    label: input.label,
  };
  return invoke(coreApiCommands.keyboardInteractiveCredentialCreate, { request });
}

export async function previewOpenSshConfig(
  configText: string,
): Promise<OpenSshConfigPreviewResponse> {
  return invoke(coreApiCommands.opensshConfigPreview, {
    request: { meta: requestMeta(), configText },
  });
}

export async function commitOpenSshConfig(
  snapshotId: string,
  candidateIds: string[],
): Promise<OpenSshConfigCommitResponse> {
  return invoke(coreApiCommands.opensshConfigCommit, {
    request: { meta: requestMeta(), snapshotId, candidateIds },
  });
}

export async function importCredential(input: {
  operationId?: string;
  idempotencyKey?: string;
  identityId: string;
  kind: CredentialKind;
  secret: string;
  passphrase?: string | null;
  priority: number;
  label: string;
}): Promise<CredentialRefSummary> {
  const operationId = input.operationId ?? createUuidV7();
  const request: CredentialImportRequest = {
    meta: requestMeta(),
    operationId,
    idempotencyKey: input.idempotencyKey ?? `credential-import-${operationId}`,
    identityId: input.identityId,
    kind: input.kind,
    secret: input.secret,
    passphrase: input.passphrase ?? null,
    priority: input.priority,
    label: input.label,
  };
  return invoke(coreApiCommands.credentialImport, {
    request: {
      ...request,
    },
  });
}

/**
 * Core owns the native file chooser and reads the selected private key directly
 * into the encrypted Vault. The WebView receives only a non-secret summary.
 */
export async function importPrivateKeyFile(input: {
  operationId?: string;
  idempotencyKey?: string;
  identityId: string;
  passphrase?: string | null;
  priority: number;
  label: string;
}): Promise<CredentialRefSummary | null> {
  const operationId = input.operationId ?? createUuidV7();
  const request: PrivateKeyFileImportRequest = {
    meta: requestMeta(),
    operationId,
    idempotencyKey: input.idempotencyKey ?? `private-key-file-import-${operationId}`,
    identityId: input.identityId,
    passphrase: input.passphrase ?? null,
    priority: input.priority,
    label: input.label,
  };
  return invoke(coreApiCommands.privateKeyFileImport, { request });
}

export async function prepareTransientCredential(input: {
  operationId?: string;
  idempotencyKey?: string;
  kind: CredentialKind;
  secret: string;
  passphrase?: string | null;
}): Promise<TransientCredentialRef> {
  const operationId = input.operationId ?? createUuidV7();
  const request: TransientCredentialPrepareRequest = {
    meta: requestMeta(),
    operationId,
    idempotencyKey: input.idempotencyKey ?? `credential-transient-${operationId}`,
    kind: input.kind,
    secret: input.secret,
    passphrase: input.passphrase ?? null,
  };
  return invoke(coreApiCommands.credentialTransientPrepare, { request });
}

export async function listKnownHosts(): Promise<KnownHostSummary[]> {
  return invoke(coreApiCommands.knownHostList, {
    request: { meta: requestMeta() },
  });
}

export async function deleteKnownHost(
  knownHostId: string,
  expectedStateVersion: string,
): Promise<KnownHostDeleteResponse> {
  return invoke(coreApiCommands.knownHostDelete, {
    request: { meta: requestMeta(), knownHostId, expectedStateVersion },
  });
}

function sshEventChannel(onEvent: (event: SshSessionEvent) => void) {
  const channel = new Channel<SshSessionEvent>();
  channel.onmessage = onEvent;
  return channel;
}

function localEventChannel(onEvent: (event: LocalSessionEvent) => void) {
  const channel = new Channel<LocalSessionEvent>();
  channel.onmessage = onEvent;
  return channel;
}

function telnetEventChannel(onEvent: (event: TelnetSessionEvent) => void) {
  const channel = new Channel<TelnetSessionEvent>();
  channel.onmessage = onEvent;
  return channel;
}

export async function openSshSession(
  input: Omit<
    SshSessionOpenRequest,
    | "meta"
    | "operationId"
    | "idempotencyKey"
    | "openAttemptId"
    | "attachAttemptId"
    | "pluginAuthorizationToken"
  > & { pluginAuthorizationToken?: string | null },
  onEvent: (event: SshSessionEvent) => void,
): Promise<SshSessionOpenResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sshTerminalOpen, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `ssh-open-${operationId}`,
      openAttemptId: createUuidV7(),
      attachAttemptId: createUuidV7(),
      ...input,
      pluginAuthorizationToken: input.pluginAuthorizationToken ?? null,
    } satisfies SshSessionOpenRequest,
    onEvent: sshEventChannel(onEvent),
  });
}

export async function testSshConnection(
  input: Omit<SshConnectionTestRequest, "meta">,
): Promise<SshConnectionTestResponse> {
  return invoke(coreApiCommands.sshConnectionTest, {
    request: { meta: requestMeta(), ...input } satisfies SshConnectionTestRequest,
  });
}

export async function fetchSshSessionSnapshot(): Promise<SshSessionSnapshot> {
  return invoke(coreApiCommands.sshTerminalSnapshot, {
    request: { meta: requestMeta() },
  });
}

export async function getSshSession(sessionId: string): Promise<SshSessionDetails> {
  return invoke(coreApiCommands.sshTerminalGet, {
    request: { meta: requestMeta(), sessionId },
  });
}

export async function attachSshSession(
  input: Omit<
    SshSessionAttachRequest,
    "meta" | "operationId" | "idempotencyKey" | "attachAttemptId"
  >,
  onEvent: (event: SshSessionEvent) => void,
): Promise<SshSessionAttachResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sshTerminalAttach, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `ssh-attach-${operationId}`,
      attachAttemptId: createUuidV7(),
      ...input,
    } satisfies SshSessionAttachRequest,
    onEvent: sshEventChannel(onEvent),
  });
}

export async function heartbeatSshAttachment(
  input: Omit<SshSessionAttachmentHeartbeatRequest, "meta">,
): Promise<SshSessionAttachment> {
  return invoke(coreApiCommands.sshTerminalAttachmentHeartbeat, {
    request: {
      meta: requestMeta(),
      ...input,
    } satisfies SshSessionAttachmentHeartbeatRequest,
  });
}

export async function detachSshSession(
  input: Omit<SshSessionDetachRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SshSessionDetachResult> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sshTerminalDetach, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `ssh-detach-${operationId}`,
      ...input,
    } satisfies SshSessionDetachRequest,
  });
}

export async function decideSshHostKey(
  input: Omit<SshHostKeyDecisionRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SshSessionDetails> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sshTerminalHostKeyDecide, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `ssh-host-key-${operationId}`,
      ...input,
    } satisfies SshHostKeyDecisionRequest,
  });
}

export async function prepareSshKeyboardInteractiveAnswer(
  input: Omit<SshKeyboardInteractiveAnswerPrepareRequest, "meta">,
): Promise<SshKeyboardInteractiveAnswerPrepareResponse> {
  return invoke(coreApiCommands.sshTerminalKeyboardInteractiveAnswerPrepare, {
    request: {
      meta: requestMeta(),
      ...input,
    } satisfies SshKeyboardInteractiveAnswerPrepareRequest,
  });
}

export async function respondSshKeyboardInteractive(
  input: Omit<SshKeyboardInteractiveResponseRequest, "meta">,
): Promise<SshSessionDetails> {
  return invoke(coreApiCommands.sshTerminalKeyboardInteractiveRespond, {
    request: {
      meta: requestMeta(),
      ...input,
    } satisfies SshKeyboardInteractiveResponseRequest,
  });
}

export async function fetchSshInputFocusSnapshot(): Promise<SshTerminalInputFocusSnapshot> {
  return invoke(coreApiCommands.sshTerminalInputFocusSnapshot, {
    request: { meta: requestMeta() },
  });
}

export async function changeSshInputFocus(
  input: Omit<
    SshTerminalInputFocusChangeRequest,
    "meta" | "operationId" | "idempotencyKey"
  >,
): Promise<SshTerminalInputFocusChangeResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sshTerminalInputFocusChange, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `ssh-input-focus-${operationId}`,
      ...input,
    } satisfies SshTerminalInputFocusChangeRequest,
  });
}

export async function fetchTerminalInputFocusSnapshot(): Promise<TerminalInputFocusSnapshot> {
  return invoke(coreApiCommands.terminalInputFocusSnapshot, {
    request: { meta: requestMeta() },
  });
}

export async function changeTerminalInputFocus(
  input: Omit<TerminalInputFocusChangeRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<TerminalInputFocusChangeResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.terminalInputFocusChange, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `terminal-input-focus-${operationId}`,
      ...input,
    } satisfies TerminalInputFocusChangeRequest,
  });
}

export async function renewSshInputLease(
  input: Omit<SshSessionInputLeaseRenewRequest, "meta">,
): Promise<SshSessionInputLease> {
  return invoke(coreApiCommands.sshTerminalInputLeaseRenew, {
    request: { meta: requestMeta(), ...input } satisfies SshSessionInputLeaseRenewRequest,
  });
}

export async function sendSshInput(
  input: Omit<SshSessionInputRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.sshTerminalInput, {
    request: { meta: requestMeta(), ...input } satisfies SshSessionInputRequest,
  });
}

export async function resizeSshTerminal(
  input: Omit<SshSessionResizeRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.sshTerminalResize, {
    request: { meta: requestMeta(), ...input } satisfies SshSessionResizeRequest,
  });
}

export async function reconnectSshSession(
  input: Omit<SshSessionReconnectRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SshSessionDetails> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sshTerminalReconnect, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `ssh-reconnect-${operationId}`,
      ...input,
    } satisfies SshSessionReconnectRequest,
  });
}

export async function takeoverSshLoginAutomation(
  input: Omit<SshLoginAutomationTakeoverRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SshLoginAutomationTakeoverResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sshTerminalLoginAutomationTakeover, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `ssh-login-automation-takeover-${operationId}`,
      ...input,
    } satisfies SshLoginAutomationTakeoverRequest,
  });
}

export async function disconnectSshSession(
  input: Omit<SshSessionDisconnectRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SshSessionDetails> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sshTerminalDisconnect, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `ssh-disconnect-${operationId}`,
      ...input,
    } satisfies SshSessionDisconnectRequest,
  });
}

export async function openLocalSession(
  input: Omit<
    LocalSessionOpenRequest,
    "meta" | "operationId" | "idempotencyKey" | "openAttemptId" | "attachAttemptId"
  >,
  onEvent: (event: LocalSessionEvent) => void,
): Promise<LocalSessionOpenResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.localTerminalOpen, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `local-open-${operationId}`,
      openAttemptId: createUuidV7(),
      attachAttemptId: createUuidV7(),
      ...input,
    } satisfies LocalSessionOpenRequest,
    onEvent: localEventChannel(onEvent),
  });
}

export async function fetchLocalSessionSnapshot(): Promise<LocalSessionSnapshot> {
  return invoke(coreApiCommands.localTerminalSnapshot, {
    request: { meta: requestMeta() },
  });
}

export async function getLocalSession(sessionId: string): Promise<LocalSessionDetails> {
  return invoke(coreApiCommands.localTerminalGet, {
    request: { meta: requestMeta(), sessionId },
  });
}

export async function attachLocalSession(
  input: Omit<
    LocalSessionAttachRequest,
    "meta" | "operationId" | "idempotencyKey" | "attachAttemptId"
  >,
  onEvent: (event: LocalSessionEvent) => void,
): Promise<LocalSessionAttachResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.localTerminalAttach, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `local-attach-${operationId}`,
      attachAttemptId: createUuidV7(),
      ...input,
    } satisfies LocalSessionAttachRequest,
    onEvent: localEventChannel(onEvent),
  });
}

export async function heartbeatLocalAttachment(
  input: Omit<LocalSessionAttachmentHeartbeatRequest, "meta">,
): Promise<LocalSessionAttachment> {
  return invoke(coreApiCommands.localTerminalAttachmentHeartbeat, {
    request: { meta: requestMeta(), ...input } satisfies LocalSessionAttachmentHeartbeatRequest,
  });
}

export async function detachLocalSession(
  input: Omit<LocalSessionDetachRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<LocalSessionDetachResult> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.localTerminalDetach, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `local-detach-${operationId}`,
      ...input,
    } satisfies LocalSessionDetachRequest,
  });
}

export async function renewLocalInputLease(
  input: Omit<LocalSessionInputLeaseRenewRequest, "meta">,
): Promise<LocalSessionInputLease> {
  return invoke(coreApiCommands.localTerminalInputLeaseRenew, {
    request: { meta: requestMeta(), ...input } satisfies LocalSessionInputLeaseRenewRequest,
  });
}

export async function sendLocalInput(
  input: Omit<LocalSessionInputRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.localTerminalInput, {
    request: { meta: requestMeta(), ...input } satisfies LocalSessionInputRequest,
  });
}

export async function resizeLocalTerminal(
  input: Omit<LocalSessionResizeRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.localTerminalResize, {
    request: { meta: requestMeta(), ...input } satisfies LocalSessionResizeRequest,
  });
}

export async function terminateLocalSession(
  input: Omit<LocalSessionTerminateRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<LocalSessionSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.localTerminalTerminate, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `local-terminate-${operationId}`,
      ...input,
    } satisfies LocalSessionTerminateRequest,
  });
}

export async function openTelnetSession(
  input: Omit<
    TelnetSessionOpenRequest,
    "meta" | "operationId" | "idempotencyKey" | "openAttemptId" | "attachAttemptId"
  >,
  onEvent: (event: TelnetSessionEvent) => void,
): Promise<TelnetSessionOpenResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.telnetTerminalOpen, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `telnet-open-${operationId}`,
      openAttemptId: createUuidV7(),
      attachAttemptId: createUuidV7(),
      ...input,
    } satisfies TelnetSessionOpenRequest,
    onEvent: telnetEventChannel(onEvent),
  });
}

export async function fetchTelnetSessionSnapshot(): Promise<TelnetSessionSnapshot> {
  return invoke(coreApiCommands.telnetTerminalSnapshot, {
    request: { meta: requestMeta() },
  });
}

export async function attachTelnetSession(
  input: Omit<
    TelnetSessionAttachRequest,
    "meta" | "operationId" | "idempotencyKey" | "attachAttemptId"
  >,
  onEvent: (event: TelnetSessionEvent) => void,
): Promise<TelnetSessionAttachResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.telnetTerminalAttach, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `telnet-attach-${operationId}`,
      attachAttemptId: createUuidV7(),
      ...input,
    } satisfies TelnetSessionAttachRequest,
    onEvent: telnetEventChannel(onEvent),
  });
}

export async function heartbeatTelnetAttachment(
  input: Omit<TelnetSessionAttachmentHeartbeatRequest, "meta">,
): Promise<TelnetSessionAttachment> {
  return invoke(coreApiCommands.telnetTerminalAttachmentHeartbeat, {
    request: { meta: requestMeta(), ...input } satisfies TelnetSessionAttachmentHeartbeatRequest,
  });
}

export async function detachTelnetSession(
  input: Omit<TelnetSessionDetachRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<TelnetSessionDetachResponse> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.telnetTerminalDetach, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `telnet-detach-${operationId}`,
      ...input,
    } satisfies TelnetSessionDetachRequest,
  });
}

export async function renewTelnetInputLease(
  input: Omit<TelnetSessionInputLeaseRenewRequest, "meta">,
): Promise<TelnetInputLease> {
  return invoke(coreApiCommands.telnetTerminalInputLeaseRenew, {
    request: { meta: requestMeta(), ...input } satisfies TelnetSessionInputLeaseRenewRequest,
  });
}

export async function sendTelnetInput(
  input: Omit<TelnetSessionInputRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.telnetTerminalInput, {
    request: { meta: requestMeta(), ...input } satisfies TelnetSessionInputRequest,
  });
}

export async function resizeTelnetTerminal(
  input: Omit<TelnetSessionResizeRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.telnetTerminalResize, {
    request: { meta: requestMeta(), ...input } satisfies TelnetSessionResizeRequest,
  });
}

export async function disconnectTelnetSession(
  input: Omit<TelnetSessionDisconnectRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<TelnetSessionSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.telnetTerminalDisconnect, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `telnet-disconnect-${operationId}`,
      ...input,
    } satisfies TelnetSessionDisconnectRequest,
  });
}

export async function reconnectTelnetSession(
  input: Omit<TelnetSessionReconnectRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<TelnetSessionSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.telnetTerminalReconnect, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `telnet-reconnect-${operationId}`,
      ...input,
    } satisfies TelnetSessionReconnectRequest,
  });
}

export async function startForwardSession(
  input: Omit<ForwardSessionStartRequest, "meta" | "operationId" | "idempotencyKey" | "sessionId">,
  onEvent: (event: ForwardSessionEvent) => void,
): Promise<ForwardSessionSummary> {
  const operationId = createUuidV7();
  const channel = new Channel<ForwardSessionEvent>();
  channel.onmessage = onEvent;
  return invoke(coreApiCommands.forwardSessionStart, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `forward-start-${operationId}`,
      sessionId: createUuidV7(),
      ...input,
    } satisfies ForwardSessionStartRequest,
    onEvent: channel,
  });
}

export async function listForwardRules(): Promise<ForwardRuleListResponse> {
  return invoke(coreApiCommands.forwardRuleList, {
    request: { meta: requestMeta() },
  });
}

export async function createForwardRule(
  input: Omit<
    ForwardRuleCreateRequest,
    "meta" | "operationId" | "idempotencyKey" | "ruleId"
  >,
): Promise<ForwardRuleSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.forwardRuleCreate, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `forward-rule-create-${operationId}`,
      ruleId: createUuidV7(),
      ...input,
    } satisfies ForwardRuleCreateRequest,
  });
}

export async function updateForwardRule(
  input: Omit<ForwardRuleUpdateRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<ForwardRuleSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.forwardRuleUpdate, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `forward-rule-update-${operationId}`,
      ...input,
    } satisfies ForwardRuleUpdateRequest,
  });
}

export async function deleteForwardRule(
  input: Omit<ForwardRuleDeleteRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<void> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.forwardRuleDelete, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `forward-rule-delete-${operationId}`,
      ...input,
    } satisfies ForwardRuleDeleteRequest,
  });
}

export async function preflightForwardRule(
  input: Omit<ForwardRulePreflightRequest, "meta">,
): Promise<ForwardRulePreflightResponse> {
  return invoke(coreApiCommands.forwardRulePreflight, {
    request: { meta: requestMeta(), ...input } satisfies ForwardRulePreflightRequest,
  });
}

export async function fetchForwardSessionSnapshot(): Promise<ForwardSessionSnapshot> {
  return invoke(coreApiCommands.forwardSessionSnapshot, {
    request: { meta: requestMeta() },
  });
}

export async function stopForwardSession(
  input: Omit<ForwardSessionStopRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<ForwardSessionSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.forwardSessionStop, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `forward-stop-${operationId}`,
      ...input,
    } satisfies ForwardSessionStopRequest,
  });
}

export async function retainForwardCleanupForExitOnce(
  input: Omit<ForwardCleanupRetainRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<ForwardSessionSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.forwardCleanupRetain, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `forward-cleanup-retain-${operationId}`,
      ...input,
    } satisfies ForwardCleanupRetainRequest,
  });
}

export async function openSftpSession(
  input: Omit<SftpSessionOpenRequest, "meta" | "operationId" | "idempotencyKey" | "sessionId">
    & { sessionId?: SftpSessionOpenRequest["sessionId"] },
): Promise<SftpSessionSummary> {
  const operationId = createUuidV7();
  const { sessionId = createUuidV7(), ...connection } = input;
  return invoke(coreApiCommands.sftpSessionOpen, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-open-${operationId}`,
      sessionId,
      ...connection,
    } satisfies SftpSessionOpenRequest,
  });
}

export async function fetchSftpSessionSnapshot(): Promise<SftpSessionSnapshot> {
  return invoke(coreApiCommands.sftpSessionSnapshot, {
    request: { meta: requestMeta() },
  });
}

export async function disconnectSftpSession(
  input: Omit<SftpSessionDisconnectRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpSessionSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpSessionDisconnect, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-disconnect-${operationId}`,
      ...input,
    } satisfies SftpSessionDisconnectRequest,
  });
}

export async function registerSftpLocalBoundary(
  kind: SftpLocalBoundaryKind,
  selectedPath: string,
): Promise<SftpLocalBoundary> {
  return invoke(coreApiCommands.sftpLocalBoundaryRegister, {
    request: { meta: requestMeta(), kind, selectedPath },
  });
}

export async function registerSftpLocalDirectory(
  selectedPath: string,
): Promise<SftpLocalDirectoryCapability> {
  return invoke(coreApiCommands.sftpLocalDirectoryRegister, {
    request: { meta: requestMeta(), selectedPath },
  });
}

export async function listSftpLocalDirectory(
  input: Omit<SftpLocalDirectoryListRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpLocalDirectoryListing> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpLocalDirectoryList, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-local-list-${operationId}`,
      ...input,
    } satisfies SftpLocalDirectoryListRequest,
  });
}

export async function openSftpLocalDirectoryChild(
  input: Omit<SftpLocalDirectoryOpenChildRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpLocalDirectoryCapability> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpLocalDirectoryOpenChild, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-local-open-child-${operationId}`,
      ...input,
    } satisfies SftpLocalDirectoryOpenChildRequest,
  });
}

export async function createSftpLocalDirectoryChild(
  input: Omit<SftpLocalDirectoryCreateChildRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpLocalDirectoryCapability> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpLocalDirectoryCreateChild, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-local-create-child-${operationId}`,
      ...input,
    } satisfies SftpLocalDirectoryCreateChildRequest,
  });
}

export async function releaseSftpLocalDirectory(
  input: Omit<SftpLocalDirectoryReleaseRequest, "meta">,
): Promise<void> {
  return invoke(coreApiCommands.sftpLocalDirectoryRelease, {
    request: { meta: requestMeta(), ...input } satisfies SftpLocalDirectoryReleaseRequest,
  });
}

export async function listSftpDirectory(
  input: Omit<SftpDirectoryListRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpDirectoryListing> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpDirectoryList, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-list-${operationId}`,
      ...input,
    } satisfies SftpDirectoryListRequest,
  });
}

export async function cancelSftpDirectoryListing(
  input: Omit<
    SftpDirectoryListCancelRequest,
    "meta" | "operationId" | "idempotencyKey"
  >,
): Promise<void> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpDirectoryListCancel, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-list-cancel-${operationId}`,
      ...input,
    } satisfies SftpDirectoryListCancelRequest,
  });
}

export async function previewSftpFile(
  input: Omit<SftpFilePreviewRequest, "meta">,
): Promise<SftpFilePreview> {
  return invoke(coreApiCommands.sftpFilePreview, {
    request: { meta: requestMeta(), ...input } satisfies SftpFilePreviewRequest,
  });
}

export async function tailSftpFile(
  input: Omit<SftpFileTailRequest, "meta">,
): Promise<SftpFileTailResult> {
  return invoke(coreApiCommands.sftpFileTail, {
    request: { meta: requestMeta(), ...input } satisfies SftpFileTailRequest,
  });
}

export async function mutateSftpFile(
  input: Omit<SftpFileMutationRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpFileMutationResult> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpFileMutate, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-file-mutate-${operationId}`,
      ...input,
    } satisfies SftpFileMutationRequest,
  });
}

export async function enqueueSftpTransfer(
  input: Omit<
    SftpTransferEnqueueRequest,
    "meta" | "operationId" | "idempotencyKey" | "transferId"
  >,
): Promise<SftpTransferSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpTransferEnqueue, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-transfer-enqueue-${operationId}`,
      transferId: createUuidV7(),
      ...input,
    } satisfies SftpTransferEnqueueRequest,
  });
}

export async function prepareSftpTransferIntent(
  input: Omit<SftpTransferIntentPrepareRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpTransferIntentPrepared> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpTransferIntentPrepare, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-intent-prepare-${operationId}`,
      ...input,
    } satisfies SftpTransferIntentPrepareRequest,
  });
}

export async function enqueueSftpTransferIntent(
  input: Omit<
    SftpTransferIntentEnqueueRequest,
    "meta" | "operationId" | "idempotencyKey" | "transferId"
  >,
): Promise<SftpTransferIntentSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpTransferIntentEnqueue, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-intent-enqueue-${operationId}`,
      transferId: createUuidV7(),
      ...input,
    } satisfies SftpTransferIntentEnqueueRequest,
  });
}

export async function fetchSftpTransferIntentSnapshot(): Promise<SftpTransferIntentSnapshot> {
  return invoke(coreApiCommands.sftpTransferIntentSnapshot, {
    request: { meta: requestMeta() },
  });
}

export async function cancelSftpTransferIntent(
  input: Omit<SftpTransferIntentActionRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpTransferIntentSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpTransferIntentCancel, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-intent-cancel-${operationId}`,
      ...input,
    } satisfies SftpTransferIntentActionRequest,
  });
}

export async function retrySftpTransferIntentCleanup(
  input: Omit<SftpTransferIntentCleanupRetryRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpTransferIntentSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpTransferIntentCleanupRetry, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-transfer-intent-cleanup-retry-${operationId}`,
      ...input,
    } satisfies SftpTransferIntentCleanupRetryRequest,
  });
}

export async function retainSftpTransferIntentCleanupForExit(
  input: Omit<SftpTransferIntentCleanupRetainRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpTransferIntentSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpTransferIntentCleanupRetain, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-transfer-intent-cleanup-retain-${operationId}`,
      ...input,
    } satisfies SftpTransferIntentCleanupRetainRequest,
  });
}

async function actOnSftpTransfer(
  command: typeof coreApiCommands.sftpTransferCancel | typeof coreApiCommands.sftpTransferResume,
  action: "cancel" | "resume",
  input: Omit<SftpTransferActionRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpTransferSummary> {
  const operationId = createUuidV7();
  return invoke(command, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-transfer-${action}-${operationId}`,
      ...input,
    } satisfies SftpTransferActionRequest,
  });
}

export async function cancelSftpTransfer(
  input: Omit<SftpTransferActionRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpTransferSummary> {
  return actOnSftpTransfer(coreApiCommands.sftpTransferCancel, "cancel", input);
}

export async function resumeSftpTransfer(
  input: Omit<SftpTransferActionRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpTransferSummary> {
  return actOnSftpTransfer(coreApiCommands.sftpTransferResume, "resume", input);
}

export async function retrySftpRemoteCleanup(
  input: Omit<SftpRemoteCleanupRetryRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpTransferSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpRemoteCleanupRetry, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-remote-cleanup-retry-${operationId}`,
      ...input,
    } satisfies SftpRemoteCleanupRetryRequest,
  });
}

export async function retainSftpRemoteCleanupForExit(
  input: Omit<SftpRemoteCleanupRetainRequest, "meta" | "operationId" | "idempotencyKey">,
): Promise<SftpTransferSummary> {
  const operationId = createUuidV7();
  return invoke(coreApiCommands.sftpRemoteCleanupRetain, {
    request: {
      meta: requestMeta(),
      operationId,
      idempotencyKey: `sftp-remote-cleanup-retain-${operationId}`,
      ...input,
    } satisfies SftpRemoteCleanupRetainRequest,
  });
}

export async function fetchTerminalWorkspaceLayout(): Promise<TerminalWorkspaceLayoutSnapshot> {
  return invoke(coreApiCommands.terminalWorkspaceLayoutGet, {
    request: { meta: requestMeta() },
  });
}

export async function replaceTerminalWorkspaceLayout(input: {
  expectedRevision: string;
  layout: TerminalWorkspaceLayout;
}): Promise<TerminalWorkspaceLayoutSnapshot> {
  return invoke(coreApiCommands.terminalWorkspaceLayoutReplace, {
    request: { meta: requestMeta(), ...input },
  });
}


export async function requestWindowClose(): Promise<ExitReadiness> {
  return invoke(coreApiCommands.windowRequestClose, {
    request: {
      meta: requestMeta(),
    },
  });
}

export async function requestApplicationExit(disconnectActiveResources = false): Promise<ExitReadiness> {
  return invoke(coreApiCommands.applicationRequestExit, {
    request: {
      meta: requestMeta(),
      disconnectActiveResources,
    },
  });
}

export function parseCoreApiError(value: unknown): CoreApiError | null {
  let candidate = value;
  if (typeof candidate === "string") {
    try {
      candidate = JSON.parse(candidate) as unknown;
    } catch {
      return null;
    }
  }
  if (typeof candidate !== "object" || candidate === null) return null;
  const error = candidate as Partial<CoreApiError>;
  return typeof error.code === "string" && typeof error.messageKey === "string"
    ? (error as CoreApiError)
    : null;
}

export function shouldRetainOperationPlan(error: CoreApiError): boolean {
  return error.retryStrategy.kind === "queryOperation";
}
