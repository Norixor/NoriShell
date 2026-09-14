export type ShortcutPlatform = "macos" | "windows";
export type ShortcutScope = "app" | "terminal";
export type ShortcutBinding = string | null;

export type ShortcutCategory = "navigation" | "workspace" | "pane" | "terminal";

export type ShortcutCommandId =
  | "navigation.overview"
  | "navigation.terminal"
  | "navigation.hosts"
  | "navigation.sftp"
  | "navigation.tunnels"
  | "navigation.plugins"
  | "navigation.settings"
  | "navigation.known-hosts"
  | "navigation.ssh-identities"
  | "workspace.tab.1"
  | "workspace.tab.2"
  | "workspace.tab.3"
  | "workspace.tab.4"
  | "workspace.tab.5"
  | "workspace.tab.6"
  | "workspace.tab.7"
  | "workspace.tab.8"
  | "workspace.tab.9"
  | "workspace.new"
  | "workspace.close"
  | "workspace.next"
  | "workspace.previous"
  | "workspace.new-local"
  | "terminal.split-right"
  | "terminal.split-down"
  | "terminal.focus-next-pane"
  | "terminal.focus-previous-pane"
  | "terminal.search"
  | "terminal.copy"
  | "terminal.paste"
  | "terminal.clear"
  | "terminal.toggle-quick-commands"
  | "terminal.history-suggestions"
  | "terminal.reconnect"
  | "terminal.disconnect"
  | "terminal.close-pane";

export interface ShortcutCommand {
  id: ShortcutCommandId;
  category: ShortcutCategory;
  scope: ShortcutScope;
  messageKey: string;
}

export type ShortcutBindings = Record<ShortcutCommandId, ShortcutBinding>;

export const SHORTCUT_PROFILE_VERSION = 1;
export const MAX_SHORTCUT_PROFILE_BYTES = 32 * 1024;

const command = (
  id: ShortcutCommandId,
  category: ShortcutCategory,
  scope: ShortcutScope,
  messageKey: string,
): ShortcutCommand => ({ id, category, scope, messageKey });

export const SHORTCUT_COMMANDS: readonly ShortcutCommand[] = [
  command("navigation.overview", "navigation", "app", "navigationOverview"),
  command("navigation.terminal", "navigation", "app", "navigationTerminal"),
  command("navigation.hosts", "navigation", "app", "navigationHosts"),
  command("navigation.sftp", "navigation", "app", "navigationSftp"),
  command("navigation.tunnels", "navigation", "app", "navigationTunnels"),
  command("navigation.plugins", "navigation", "app", "navigationPlugins"),
  command("navigation.settings", "navigation", "app", "navigationSettings"),
  command("navigation.known-hosts", "navigation", "app", "navigationKnownHosts"),
  command("navigation.ssh-identities", "navigation", "app", "navigationSshIdentities"),
  command("workspace.tab.1", "workspace", "app", "workspaceTab1"),
  command("workspace.tab.2", "workspace", "app", "workspaceTab2"),
  command("workspace.tab.3", "workspace", "app", "workspaceTab3"),
  command("workspace.tab.4", "workspace", "app", "workspaceTab4"),
  command("workspace.tab.5", "workspace", "app", "workspaceTab5"),
  command("workspace.tab.6", "workspace", "app", "workspaceTab6"),
  command("workspace.tab.7", "workspace", "app", "workspaceTab7"),
  command("workspace.tab.8", "workspace", "app", "workspaceTab8"),
  command("workspace.tab.9", "workspace", "app", "workspaceTab9"),
  command("workspace.new", "workspace", "app", "workspaceNew"),
  command("workspace.close", "workspace", "app", "workspaceClose"),
  command("workspace.next", "workspace", "app", "workspaceNext"),
  command("workspace.previous", "workspace", "app", "workspacePrevious"),
  command("workspace.new-local", "workspace", "app", "workspaceNewLocal"),
  command("terminal.split-right", "pane", "terminal", "splitRight"),
  command("terminal.split-down", "pane", "terminal", "splitDown"),
  command("terminal.focus-next-pane", "pane", "terminal", "focusNextPane"),
  command("terminal.focus-previous-pane", "pane", "terminal", "focusPreviousPane"),
  command("terminal.search", "terminal", "terminal", "search"),
  command("terminal.copy", "terminal", "terminal", "copy"),
  command("terminal.paste", "terminal", "terminal", "paste"),
  command("terminal.clear", "terminal", "terminal", "clear"),
  command("terminal.toggle-quick-commands", "terminal", "terminal", "toggleQuickCommands"),
  command("terminal.history-suggestions", "terminal", "terminal", "historySuggestions"),
  command("terminal.reconnect", "terminal", "terminal", "reconnect"),
  command("terminal.disconnect", "terminal", "terminal", "disconnect"),
  command("terminal.close-pane", "terminal", "terminal", "closePane"),
] as const;

const commandById = new Map(SHORTCUT_COMMANDS.map((item) => [item.id, item]));

const MAC_DEFAULTS: ShortcutBindings = {
  "navigation.overview": "Meta+Shift+KeyO",
  "navigation.terminal": "Meta+Shift+KeyT",
  "navigation.hosts": "Meta+Shift+KeyH",
  "navigation.sftp": "Meta+Shift+KeyS",
  "navigation.tunnels": "Meta+Shift+KeyU",
  "navigation.plugins": "Meta+Shift+KeyL",
  "navigation.settings": "Meta+Comma",
  "navigation.known-hosts": "Meta+Shift+KeyK",
  "navigation.ssh-identities": "Meta+Shift+KeyI",
  "workspace.tab.1": "Meta+Digit1",
  "workspace.tab.2": "Meta+Digit2",
  "workspace.tab.3": "Meta+Digit3",
  "workspace.tab.4": "Meta+Digit4",
  "workspace.tab.5": "Meta+Digit5",
  "workspace.tab.6": "Meta+Digit6",
  "workspace.tab.7": "Meta+Digit7",
  "workspace.tab.8": "Meta+Digit8",
  "workspace.tab.9": "Meta+Digit9",
  "workspace.new": "Meta+KeyT",
  "workspace.close": "Meta+KeyW",
  "workspace.next": "Meta+Alt+ArrowRight",
  "workspace.previous": "Meta+Alt+ArrowLeft",
  "workspace.new-local": null,
  "terminal.split-right": "Meta+KeyD",
  "terminal.split-down": "Meta+Shift+KeyD",
  "terminal.focus-next-pane": "Meta+Alt+ArrowDown",
  "terminal.focus-previous-pane": "Meta+Alt+ArrowUp",
  "terminal.search": "Meta+KeyF",
  "terminal.copy": "Meta+Shift+KeyC",
  "terminal.paste": "Meta+Shift+KeyV",
  "terminal.clear": "Meta+KeyK",
  "terminal.toggle-quick-commands": "Meta+Shift+KeyP",
  "terminal.history-suggestions": "Meta+Shift+KeyY",
  "terminal.reconnect": "Meta+Shift+KeyR",
  "terminal.disconnect": "Meta+Shift+KeyE",
  "terminal.close-pane": "Meta+Shift+KeyW",
};

const WINDOWS_DEFAULTS: ShortcutBindings = {
  "navigation.overview": "Ctrl+Shift+KeyO",
  "navigation.terminal": "Ctrl+Shift+KeyT",
  "navigation.hosts": "Ctrl+Shift+KeyH",
  "navigation.sftp": "Ctrl+Shift+KeyS",
  "navigation.tunnels": "Ctrl+Shift+KeyU",
  "navigation.plugins": "Ctrl+Shift+KeyL",
  "navigation.settings": "Ctrl+Comma",
  "navigation.known-hosts": "Ctrl+Shift+KeyK",
  "navigation.ssh-identities": "Ctrl+Shift+KeyI",
  "workspace.tab.1": "Ctrl+Digit1",
  "workspace.tab.2": "Ctrl+Digit2",
  "workspace.tab.3": "Ctrl+Digit3",
  "workspace.tab.4": "Ctrl+Digit4",
  "workspace.tab.5": "Ctrl+Digit5",
  "workspace.tab.6": "Ctrl+Digit6",
  "workspace.tab.7": "Ctrl+Digit7",
  "workspace.tab.8": "Ctrl+Digit8",
  "workspace.tab.9": "Ctrl+Digit9",
  "workspace.new": "Ctrl+KeyT",
  "workspace.close": "Ctrl+KeyW",
  "workspace.next": "Ctrl+Alt+ArrowRight",
  "workspace.previous": "Ctrl+Alt+ArrowLeft",
  "workspace.new-local": null,
  "terminal.split-right": "Ctrl+KeyD",
  "terminal.split-down": "Ctrl+Shift+KeyD",
  "terminal.focus-next-pane": "Ctrl+Alt+ArrowDown",
  "terminal.focus-previous-pane": "Ctrl+Alt+ArrowUp",
  "terminal.search": "Ctrl+KeyF",
  "terminal.copy": "Ctrl+Shift+KeyC",
  "terminal.paste": "Ctrl+Shift+KeyV",
  "terminal.clear": "Ctrl+KeyL",
  "terminal.toggle-quick-commands": "Ctrl+Shift+KeyP",
  "terminal.history-suggestions": "Ctrl+Shift+KeyY",
  "terminal.reconnect": "Ctrl+Shift+KeyR",
  "terminal.disconnect": "Ctrl+Shift+KeyE",
  "terminal.close-pane": "Ctrl+Shift+KeyW",
};

const modifierOrder = ["Meta", "Ctrl", "Alt", "Shift"] as const;
const supportedCodes = new Set<string>([
  ...Array.from({ length: 26 }, (_, index) => `Key${String.fromCharCode(65 + index)}`),
  ...Array.from({ length: 10 }, (_, index) => `Digit${index}`),
  ...Array.from({ length: 12 }, (_, index) => `F${index + 1}`),
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Enter",
  "Escape",
  "Space",
  "Tab",
  "Backspace",
  "Delete",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "BracketLeft",
  "BracketRight",
  "Comma",
  "Period",
  "Slash",
  "Minus",
  "Equal",
  "Quote",
  "Backquote",
  "Semicolon",
]);

const keyAliases: Record<string, string> = {
  cmd: "Meta",
  command: "Meta",
  meta: "Meta",
  ctrl: "Ctrl",
  control: "Ctrl",
  alt: "Alt",
  option: "Alt",
  shift: "Shift",
  up: "ArrowUp",
  down: "ArrowDown",
  left: "ArrowLeft",
  right: "ArrowRight",
  esc: "Escape",
  escape: "Escape",
  space: "Space",
  return: "Enter",
  enter: "Enter",
  del: "Delete",
  delete: "Delete",
  backspace: "Backspace",
  home: "Home",
  end: "End",
  pageup: "PageUp",
  pagedown: "PageDown",
  comma: "Comma",
  period: "Period",
  slash: "Slash",
  minus: "Minus",
  equal: "Equal",
  quote: "Quote",
  backquote: "Backquote",
  semicolon: "Semicolon",
  tab: "Tab",
};

export type ShortcutValidationError =
  | "invalid"
  | "missing-modifier"
  | "platform-modifier"
  | "system-reserved";

export interface ShortcutValidationResult {
  binding: ShortcutBinding;
  error: ShortcutValidationError | null;
}

export interface ShortcutProfile {
  version: typeof SHORTCUT_PROFILE_VERSION;
  bindings: Record<ShortcutPlatform, ShortcutBindings>;
}

export type ShortcutProfileParseResult =
  | { ok: true; profile: ShortcutProfile }
  | { ok: false; error: "invalid-json" | "invalid-profile" | "too-large" | "conflict" };

export interface ShortcutContext {
  terminalActive?: boolean;
  modalOpen?: boolean;
  editableTarget?: boolean;
  shortcutRecording?: boolean;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function normalizeCode(token: string) {
  const trimmed = token.trim();
  const alias = keyAliases[trimmed.toLowerCase()];
  if (alias) return alias;
  if (/^[a-z]$/i.test(trimmed)) return `Key${trimmed.toUpperCase()}`;
  if (/^[0-9]$/.test(trimmed)) return `Digit${trimmed}`;
  if (/^key[a-z]$/i.test(trimmed)) return `Key${trimmed.at(-1)?.toUpperCase()}`;
  if (/^digit[0-9]$/i.test(trimmed)) return `Digit${trimmed.at(-1)}`;
  if (/^f(?:[1-9]|1[0-2])$/i.test(trimmed)) return trimmed.toUpperCase();
  return trimmed;
}

function parseShortcutParts(value: string) {
  const tokens = value.split("+").map((token) => token.trim()).filter(Boolean);
  if (tokens.length === 0 || tokens.length > modifierOrder.length + 1) return null;
  const modifiers = new Set<string>();
  let code: string | null = null;
  for (const token of tokens) {
    const normalized = normalizeCode(token);
    if ((modifierOrder as readonly string[]).includes(normalized)) {
      if (modifiers.has(normalized)) return null;
      modifiers.add(normalized);
      continue;
    }
    if (code !== null || !supportedCodes.has(normalized)) return null;
    code = normalized;
  }
  if (!code || modifiers.size === 0) return null;
  return { modifiers, code };
}

function buildBinding(modifiers: ReadonlySet<string>, code: string) {
  return [...modifierOrder.filter((modifier) => modifiers.has(modifier)), code].join("+");
}

export function getShortcutCommand(commandId: ShortcutCommandId) {
  return commandById.get(commandId) ?? null;
}

export function createDefaultShortcutBindings(platform: ShortcutPlatform): ShortcutBindings {
  return { ...(platform === "macos" ? MAC_DEFAULTS : WINDOWS_DEFAULTS) };
}

export function validateShortcutBinding(
  value: unknown,
  platform: ShortcutPlatform,
  scope: ShortcutScope,
): ShortcutValidationResult {
  if (value === null) return { binding: null, error: null };
  if (typeof value !== "string" || value.length > 96) return { binding: null, error: "invalid" };
  const parsed = parseShortcutParts(value);
  if (!parsed) return { binding: null, error: "missing-modifier" };
  if (platform === "windows" && parsed.modifiers.has("Meta")) {
    return { binding: null, error: "platform-modifier" };
  }
  if (scope === "app" && !(platform === "macos" ? parsed.modifiers.has("Meta") : parsed.modifiers.has("Ctrl"))) {
    return { binding: null, error: "missing-modifier" };
  }
  const binding = buildBinding(parsed.modifiers, parsed.code);
  if (isSystemReservedShortcut(binding, platform)) return { binding: null, error: "system-reserved" };
  return { binding, error: null };
}

export function normalizeShortcutBinding(
  value: unknown,
  platform: ShortcutPlatform,
  scope: ShortcutScope,
): ShortcutBinding | null {
  const result = validateShortcutBinding(value, platform, scope);
  return result.error ? null : result.binding;
}

export function isSystemReservedShortcut(binding: string, platform: ShortcutPlatform) {
  const parsed = parseShortcutParts(binding);
  if (!parsed) return true;
  const { modifiers, code } = parsed;
  if (platform === "windows") {
    if (modifiers.has("Meta")) return true;
    if (modifiers.has("Ctrl") && !modifiers.has("Alt") && !modifiers.has("Shift") && ["KeyC", "KeyV", "KeyX", "KeyZ", "KeyA", "KeyY", "KeyM"].includes(code)) return true;
    return (
      (modifiers.has("Alt") && (code === "F4" || code === "Tab" || code === "Escape"))
      || (modifiers.has("Ctrl") && modifiers.has("Alt") && code === "Delete")
      || (modifiers.has("Ctrl") && modifiers.has("Shift") && code === "Escape")
    );
  }
  const plainMeta = modifiers.has("Meta")
    && !modifiers.has("Ctrl")
    && !modifiers.has("Alt")
    && !modifiers.has("Shift");
  return (
    (plainMeta && ["KeyQ", "KeyH", "KeyM", "KeyC", "KeyV", "KeyX", "KeyZ", "KeyA", "Tab", "Space"].includes(code))
    // The native macOS Edit menu handles undo, redo, and clipboard first; web bindings cannot reassign them.
    || (modifiers.has("Meta") && modifiers.has("Shift") && !modifiers.has("Alt") && !modifiers.has("Ctrl") && code === "KeyZ")
    || (modifiers.has("Meta") && modifiers.has("Alt") && code === "Escape")
    || (modifiers.has("Ctrl") && modifiers.has("Meta") && ["KeyQ", "KeyF"].includes(code))
    || (modifiers.has("Meta") && modifiers.has("Alt") && !modifiers.has("Ctrl") && !modifiers.has("Shift") && code === "KeyH")
  );
}

function eventCode(event: KeyboardEvent) {
  if (supportedCodes.has(event.code)) return event.code;
  const key = event.key;
  if (key.length === 1 && /^[a-z]$/i.test(key)) return `Key${key.toUpperCase()}`;
  if (key.length === 1 && /^[0-9]$/.test(key)) return `Digit${key}`;
  return normalizeCode(key);
}

export function shortcutFromKeyboardEvent(
  event: KeyboardEvent,
  platform: ShortcutPlatform,
  scope: ShortcutScope,
): ShortcutValidationResult {
  if (event.isComposing || event.repeat) return { binding: null, error: "invalid" };
  const code = eventCode(event);
  if (!supportedCodes.has(code)) return { binding: null, error: "invalid" };
  const modifiers = new Set<string>();
  if (event.metaKey) modifiers.add("Meta");
  if (event.ctrlKey) modifiers.add("Ctrl");
  if (event.altKey) modifiers.add("Alt");
  if (event.shiftKey) modifiers.add("Shift");
  return validateShortcutBinding(buildBinding(modifiers, code), platform, scope);
}

export function formatShortcutBinding(binding: ShortcutBinding, platform: ShortcutPlatform) {
  if (!binding) return "";
  const parsed = parseShortcutParts(binding);
  if (!parsed) return binding;
  const keyLabels: Record<string, string> = {
    ArrowUp: "↑",
    ArrowDown: "↓",
    ArrowLeft: "←",
    ArrowRight: "→",
    Escape: "Esc",
    Space: "Space",
    Backspace: "Backspace",
    PageUp: "Page Up",
    PageDown: "Page Down",
    BracketLeft: "[",
    BracketRight: "]",
    Comma: ",",
    Period: ".",
    Slash: "/",
    Minus: "-",
    Equal: "=",
    Quote: "'",
    Backquote: "`",
    Semicolon: ";",
  };
  const key = parsed.code.startsWith("Key")
    ? parsed.code.slice(3)
    : parsed.code.startsWith("Digit")
      ? parsed.code.slice(5)
      : keyLabels[parsed.code] ?? parsed.code;
  if (platform === "macos") {
    const labels: Record<string, string> = { Meta: "⌘", Ctrl: "⌃", Alt: "⌥", Shift: "⇧" };
    return `${modifierOrder.filter((modifier) => parsed.modifiers.has(modifier)).map((modifier) => labels[modifier]).join("")}${key}`;
  }
  const labels: Record<string, string> = { Meta: "Win", Ctrl: "Ctrl", Alt: "Alt", Shift: "Shift" };
  return [...modifierOrder.filter((modifier) => parsed.modifiers.has(modifier)).map((modifier) => labels[modifier]), key].join("+");
}

export function isEditableShortcutTarget(target: EventTarget | null) {
  if (!(target instanceof Element)) return false;
  // xterm owns this hidden textarea for terminal input. Treating it as a form field
  // would make every terminal-scoped shortcut unavailable while xterm has focus.
  if (target.closest(".xterm-helper-textarea, .xterm textarea, [data-desktop-input]")) return false;
  return Boolean(target.closest("input, textarea, select, [contenteditable='true'], [role='textbox']"));
}

export function matchesShortcut(event: KeyboardEvent, binding: ShortcutBinding) {
  if (!binding || event.isComposing) return false;
  const parsed = parseShortcutParts(binding);
  if (!parsed || eventCode(event) !== parsed.code) return false;
  return event.metaKey === parsed.modifiers.has("Meta")
    && event.ctrlKey === parsed.modifiers.has("Ctrl")
    && event.altKey === parsed.modifiers.has("Alt")
    && event.shiftKey === parsed.modifiers.has("Shift");
}

export function findShortcutConflicts(
  bindings: ShortcutBindings,
  commandId: ShortcutCommandId,
  binding: ShortcutBinding,
) {
  if (!binding) return [];
  return SHORTCUT_COMMANDS.filter((candidate) => (
    candidate.id !== commandId && bindings[candidate.id] === binding
  ));
}

export function matchShortcut(
  event: KeyboardEvent,
  platform: ShortcutPlatform,
  bindings: ShortcutBindings,
): ShortcutCommand | null {
  if (platform === "windows" && event.metaKey) return null;
  for (const candidate of SHORTCUT_COMMANDS) {
    if (!matchesShortcut(event, bindings[candidate.id])) continue;
    return candidate;
  }
  return null;
}

export function isShortcutExecutionAllowed(
  command: ShortcutCommand,
  event: KeyboardEvent,
  context: ShortcutContext = {},
) {
  if (event.repeat || context.modalOpen || context.shortcutRecording) return false;
  if (context.editableTarget || isEditableShortcutTarget(event.target)) return false;
  return command.scope !== "terminal" || context.terminalActive === true;
}

export function shouldConsumeShortcut(command: ShortcutCommand | null, context: ShortcutContext = {}) {
  // A recorder needs the event to reach its dialog-local capture handler; all other
  // matched commands stay reserved even when execution is blocked by a dialog/repeat.
  return command !== null && !context.shortcutRecording;
}

export function resolveShortcut(
  event: KeyboardEvent,
  platform: ShortcutPlatform,
  bindings: ShortcutBindings,
  context: ShortcutContext = {},
): ShortcutCommand | null {
  const command = matchShortcut(event, platform, bindings);
  return command && isShortcutExecutionAllowed(command, event, context) ? command : null;
}

function validateBindingSet(platform: ShortcutPlatform, value: unknown): ShortcutBindings | null {
  if (!isRecord(value) || Object.keys(value).length !== SHORTCUT_COMMANDS.length) return null;
  const bindings = {} as ShortcutBindings;
  for (const item of SHORTCUT_COMMANDS) {
    if (!(item.id in value)) return null;
    const result = validateShortcutBinding(value[item.id], platform, item.scope);
    if (result.error) return null;
    bindings[item.id] = result.binding;
  }
  const seen = new Set<string>();
  for (const item of SHORTCUT_COMMANDS) {
    const binding = bindings[item.id];
    if (!binding) continue;
    if (seen.has(binding)) return null;
    seen.add(binding);
  }
  return bindings;
}

export function serializeShortcutProfile(profile: ShortcutProfile) {
  return JSON.stringify(profile);
}

export function parseShortcutProfile(text: string): ShortcutProfileParseResult {
  if (new TextEncoder().encode(text).byteLength > MAX_SHORTCUT_PROFILE_BYTES) {
    return { ok: false, error: "too-large" };
  }
  let raw: unknown;
  try {
    raw = JSON.parse(text) as unknown;
  } catch {
    return { ok: false, error: "invalid-json" };
  }
  if (!isRecord(raw) || raw.version !== SHORTCUT_PROFILE_VERSION || !isRecord(raw.bindings)) {
    return { ok: false, error: "invalid-profile" };
  }
  const macos = validateBindingSet("macos", raw.bindings.macos);
  const windows = validateBindingSet("windows", raw.bindings.windows);
  if (!macos || !windows) return { ok: false, error: "invalid-profile" };
  return {
    ok: true,
    profile: {
      version: SHORTCUT_PROFILE_VERSION,
      bindings: { macos, windows },
    },
  };
}
