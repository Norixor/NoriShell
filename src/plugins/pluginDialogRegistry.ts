/**
  * Trusted owner for declarative plugin dialogs. DOM protection attributes are deliberately not trusted; only the host
  * renderer can register the actual dialog-content node.
 */
export type PluginDialogOwner = Readonly<{
  pluginId: string;
  packageSha256: string;
  instanceGeneration: string;
}>;

export type RegisteredPluginDialog = Readonly<{
  content: HTMLElement;
  owner: PluginDialogOwner;
  close: () => void;
}>;

export type PluginDialogState = Readonly<{
  dialog: HTMLElement;
  registration: RegisteredPluginDialog | null;
}>;

const dialogSelector = '[role="dialog"], dialog[open]';
const registrations = new Map<HTMLElement, RegisteredPluginDialog>();

export function pluginDialogOwnerMatches(left: PluginDialogOwner, right: PluginDialogOwner): boolean {
  return left.pluginId === right.pluginId
    && left.packageSha256 === right.packageSha256
    && left.instanceGeneration === right.instanceGeneration;
}

/**
  * Registers the renderer content node of an open dialog. Replacement wins, and an old disposer
  * cannot remove the replacement registration.
 */
export function registerPluginDialog(
  content: HTMLElement,
  owner: PluginDialogOwner,
  close: () => void,
): () => void {
  const registration: RegisteredPluginDialog = {
    content,
    owner: {
      pluginId: owner.pluginId,
      packageSha256: owner.packageSha256,
      instanceGeneration: owner.instanceGeneration,
    },
    close,
  };
  registrations.set(content, registration);
  return () => {
    if (registrations.get(content) === registration) registrations.delete(content);
  };
}

function removeDisconnectedRegistrations() {
  for (const [content, registration] of registrations) {
    if (content.isConnected || registrations.get(content) !== registration) continue;
    registrations.delete(content);
  }
}

function registrationForDialog(dialog: HTMLElement): RegisteredPluginDialog | null {
  const matches = [...registrations.values()].filter((registration) => (
    registration.content.closest<HTMLElement>(dialogSelector) === dialog
  ));
  return matches.length === 1 ? matches[0] ?? null : null;
}

/**
  * Returns every dialog still in the DOM. Unregistered or ambiguously owned dialogs remain null so callers
  * fail closed instead of trusting declarative DOM attributes.
 */
export function listPluginDialogs(): PluginDialogState[] {
  removeDisconnectedRegistrations();
  return [...document.querySelectorAll<HTMLElement>(dialogSelector)].map((dialog) => ({
    dialog,
    registration: registrationForDialog(dialog),
  }));
}
