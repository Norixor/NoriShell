import { invoke } from "@tauri-apps/api/core";

import type { ThemeDefinition } from "../app-theme";
import type { ThemePackageEntry as CoreThemePackageEntry } from "./generated/core-api";

/** Core only returns definitions from current, verified installed packages. */
export interface ThemePackageEntry extends Omit<CoreThemePackageEntry, "definition"> {
  definition: ThemeDefinition;
}

export interface ThemePackageListResponse {
  themes: ThemePackageEntry[];
}

export function listPluginThemes(): Promise<ThemePackageListResponse> {
  return invoke<ThemePackageListResponse>("plugin_theme_list", {
    request: { meta: { requestId: crypto.randomUUID() } },
  });
}
