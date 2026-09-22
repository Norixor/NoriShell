import { fetchVaultStatus } from "./core-api/client";
import { requestSecureVault } from "./core-api/secure-vault-client";
import type { VaultState } from "./core-api/generated/core-api";
import type { ShowNvxTipInput } from "./stores/tips";

export const STARTUP_VAULT_TIP_SCOPE = "startup-vault";

export interface StartupVaultTipStore {
  show(input: ShowNvxTipInput): string;
  dismissScope(scope: string): void;
}

export interface StartupVaultTipOptions {
  t: (key: string) => string;
  tips: StartupVaultTipStore;
  fetchStatus?: () => Promise<{ state: VaultState }>;
  requestUnlock?: () => Promise<boolean>;
  eventTarget?: Pick<Window, "addEventListener" | "removeEventListener"> | null;
}

function needsManualUnlock(state: VaultState) {
  return state === "locked" || state === "requiresReload";
}

/**
 * Publishes a single startup reminder. It never opens a protected Vault window
 * until the user explicitly chooses the action after a fresh Core state check.
 */
export function initializeStartupVaultTip(options: StartupVaultTipOptions) {
  const fetchStatus = options.fetchStatus ?? fetchVaultStatus;
  const requestUnlock = options.requestUnlock ?? (() => requestSecureVault("ensureUnlocked"));
  const eventTarget = options.eventTarget ?? (typeof window === "undefined" ? null : window);
  let disposed = false;
  let statusGeneration = 0;
  let unlockInFlight = false;

  async function currentState(reportFailure = false) {
    const generation = ++statusGeneration;
    let state: VaultState;
    try {
      state = (await fetchStatus()).state;
    } catch (error) {
      if (reportFailure) throw error;
      return null;
    }
    return disposed || generation !== statusGeneration ? null : state;
  }

  async function dismissWhenResolved() {
    const state = await currentState();
    if (state && !needsManualUnlock(state)) {
      options.tips.dismissScope(STARTUP_VAULT_TIP_SCOPE);
    }
  }

  async function showInitialReminder() {
    const state = await currentState();
    if (!state || !needsManualUnlock(state)) {
      if (state) options.tips.dismissScope(STARTUP_VAULT_TIP_SCOPE);
      return;
    }
    options.tips.show({
      scope: STARTUP_VAULT_TIP_SCOPE,
      tone: "warning",
      title: options.t("tips.startupVault.title"),
      message: options.t("tips.startupVault.description"),
      durationMs: 10_000,
      action: {
        label: options.t("tips.startupVault.unlockNow"),
        onClick: async () => {
          if (disposed || unlockInFlight) return;
          unlockInFlight = true;
          try {
            const current = await currentState(true);
            if (!current) return;
            if (!needsManualUnlock(current)) {
              options.tips.dismissScope(STARTUP_VAULT_TIP_SCOPE);
              return;
            }
            if (await requestUnlock()) await dismissWhenResolved();
          } catch {
            if (!disposed) {
              options.tips.show({
                scope: "startup-vault-action",
                tone: "error",
                title: options.t("tips.startupVault.unlockFailed"),
              });
            }
          } finally {
            unlockInFlight = false;
          }
        },
      },
    });
  }

  const onVaultChanged = () => { void dismissWhenResolved(); };
  eventTarget?.addEventListener("norishell:vault-changed", onVaultChanged);
  void showInitialReminder();

  return () => {
    if (disposed) return;
    disposed = true;
    statusGeneration += 1;
    eventTarget?.removeEventListener("norishell:vault-changed", onVaultChanged);
    options.tips.dismissScope(STARTUP_VAULT_TIP_SCOPE);
  };
}
