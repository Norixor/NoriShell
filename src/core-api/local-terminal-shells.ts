import { invoke, isTauri } from "@tauri-apps/api/core";

export interface LocalTerminalShellOption {
  /** Fixed Windows id or `path:<absolute path>`; the value stored in the application preference. */
  id: string;
  name: string;
  path: string;
}

export interface LocalTerminalShellCatalog {
  /** Name of the shell that "system default" resolves to on this machine. */
  defaultShellName: string;
  options: LocalTerminalShellOption[];
}

const EMPTY_CATALOG: LocalTerminalShellCatalog = { defaultShellName: "", options: [] };

/** Reads the detected shells; browser previews and an unavailable command yield an empty catalog. */
export async function getLocalTerminalShellCatalog(): Promise<LocalTerminalShellCatalog> {
  if (!isTauri()) return { ...EMPTY_CATALOG };
  try {
    const catalog = await invoke<LocalTerminalShellCatalog>("local_terminal_shell_catalog");
    if (!catalog || typeof catalog.defaultShellName !== "string" || !Array.isArray(catalog.options)) return { ...EMPTY_CATALOG };
    return {
      defaultShellName: catalog.defaultShellName,
      options: catalog.options.filter((option) => typeof option?.id === "string" && typeof option.name === "string"),
    };
  } catch {
    return { ...EMPTY_CATALOG };
  }
}
