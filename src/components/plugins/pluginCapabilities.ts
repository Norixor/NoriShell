import type { PluginCapability } from "../../core-api/generated/core-api";

// Keep the UI grouping aligned with Core's special_plugin_capability installation approval categories; this only selects the UI entry point,
// while Core's protected window and installation-commit fence still validate the final authorization.
export const specialPluginCapabilities: ReadonlySet<PluginCapability> = new Set([
  "uiWebviewIsolated",
  "networkDomain",
  "localFiles",
  "localProcess",
  "deviceSerial",
  "terminalProvider",
  "credentialsPlugin",
  "uiHostDomObserve",
  "uiHostDomMutate",
  "uiHostCss",
  "hostMetadataRead",
  "hostMutationPropose",
  "hostSessionRequest",
  "remoteInspect",
  "remoteExecRequest",
  "sftpRead",
  "sftpWrite",
  "sshSync",
  "appPreferencesRead",
  "terminalHistoryRead",
]);

export const isSpecialPluginCapability = (capability: PluginCapability) => specialPluginCapabilities.has(capability);
