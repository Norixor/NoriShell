import { invoke, isTauri } from "@tauri-apps/api/core";

export type JsonExportKind = "preferences" | "shortcuts" | "theme";

const NAMES: Record<JsonExportKind, string> = {
  theme: "norishell-theme-profile-v1.json",
  preferences: "norishell-preferences-v1.json",
  shortcuts: "norishell-shortcuts.json",
};

/** Core reports this when a picked document was truncated before the write failed, so the file may be partial. */
export const JSON_EXPORT_WRITE_INCOMPLETE = "native-json-export-write-incomplete";

export function isJsonExportWriteIncomplete(error: unknown): boolean {
  return error === JSON_EXPORT_WRITE_INCOMPLETE || (error instanceof Error && error.message === JSON_EXPORT_WRITE_INCOMPLETE);
}

/** Tauri delegates saving to Core; browser previews continue to use standard Blob downloads. */
export async function exportJsonFile(kind: JsonExportKind, text: string): Promise<boolean> {
  if (isTauri()) {
    return invoke<boolean>("native_json_export", { kind, text });
  }

  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = NAMES[kind];
  anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
  return true;
}
