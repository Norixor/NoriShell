import { defineStore } from "pinia";
import { ref } from "vue";

const QUICK_COMMANDS_KEY = "norishell.quick-commands.v1";
const MAX_QUICK_COMMANDS = 200;
export const MAX_QUICK_COMMAND_LABEL_LENGTH = 80;
export const MAX_QUICK_COMMAND_LENGTH = 4_096;

export type QuickCommandSaveResult =
  | "saved"
  | "invalid"
  | "too-long"
  | "sensitive"
  | "limit-reached"
  | "storage-error";

export type QuickCommandRemoveResult = "removed" | "not-found" | "storage-error";

export interface QuickCommand {
  id: string;
  label: string;
  command: string;
  createdAt: string;
  updatedAt: string;
}

function normalizeText(value: unknown) {
  return typeof value === "string" ? value.trim() : "";
}

function containsSensitiveMaterial(value: string) {
  return [
    /-----BEGIN (?:OPENSSH |RSA |EC |DSA |ENCRYPTED )?PRIVATE KEY-----/i,
    /authorization\s*:\s*bearer\s+\S+/i,
    /\b(?:gh[pousr]_[a-z0-9]{20,}|sk-[a-z0-9_-]{20,}|AKIA[A-Z0-9]{16})\b/i,
    /(?:^|\s)(?:password|passwd|token|secret|api[_-]?key)\s*=\s*["']?(?!\$)\S+/i,
  ].some((pattern) => pattern.test(value));
}

function parseQuickCommand(value: unknown): QuickCommand | null {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Partial<QuickCommand>;
  const label = normalizeText(candidate.label);
  const command = normalizeText(candidate.command);
  if (
    typeof candidate.id !== "string"
    || !candidate.id
    || !label
    || !command
    || label.length > MAX_QUICK_COMMAND_LABEL_LENGTH
    || command.length > MAX_QUICK_COMMAND_LENGTH
    || containsSensitiveMaterial(command)
    || typeof candidate.createdAt !== "string"
    || typeof candidate.updatedAt !== "string"
  ) {
    return null;
  }
  return {
    id: candidate.id,
    label,
    command,
    createdAt: candidate.createdAt,
    updatedAt: candidate.updatedAt,
  };
}

function readQuickCommands(): QuickCommand[] {
  try {
    const value = JSON.parse(localStorage.getItem(QUICK_COMMANDS_KEY) ?? "[]") as unknown;
    if (!Array.isArray(value)) return [];
    return value
      .map(parseQuickCommand)
      .filter((command): command is QuickCommand => command !== null)
      .slice(0, MAX_QUICK_COMMANDS);
  } catch {
    return [];
  }
}

export const useQuickCommandsStore = defineStore("quickCommands", () => {
  const commands = ref<QuickCommand[]>(readQuickCommands());

  function persist(next: QuickCommand[]) {
    try {
      localStorage.setItem(QUICK_COMMANDS_KEY, JSON.stringify(next));
      return true;
    } catch {
      return false;
    }
  }

  function save(input: { id?: string; label: string; command: string }): QuickCommandSaveResult {
    const label = normalizeText(input.label);
    const command = normalizeText(input.command);
    if (!label || !command) return "invalid";
    if (
      label.length > MAX_QUICK_COMMAND_LABEL_LENGTH
      || command.length > MAX_QUICK_COMMAND_LENGTH
    ) return "too-long";
    if (containsSensitiveMaterial(command)) return "sensitive";
    const now = new Date().toISOString();
    const existingIndex = input.id
      ? commands.value.findIndex((item) => item.id === input.id)
      : -1;
    if (existingIndex >= 0) {
      const existing = commands.value[existingIndex];
      if (!existing) return "invalid";
      const next = [...commands.value];
      next.splice(existingIndex, 1, {
        ...existing,
        label,
        command,
        updatedAt: now,
      });
      if (!persist(next)) return "storage-error";
      commands.value = next;
    } else {
      if (commands.value.length >= MAX_QUICK_COMMANDS) return "limit-reached";
      const next = [{
        id: crypto.randomUUID(),
        label,
        command,
        createdAt: now,
        updatedAt: now,
      }, ...commands.value];
      if (!persist(next)) return "storage-error";
      commands.value = next;
    }
    return "saved";
  }

  function remove(id: string): QuickCommandRemoveResult {
    const next = commands.value.filter((command) => command.id !== id);
    if (next.length === commands.value.length) return "not-found";
    if (!persist(next)) return "storage-error";
    commands.value = next;
    return "removed";
  }

  return { commands, save, remove };
});
