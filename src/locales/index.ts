import { createI18n } from "vue-i18n";

import { releasesEn, releasesZhCN } from "./releases";
import { secureWindowEn, secureWindowZhCN } from "./secure-windows";
import { sftpToolWindowEn, sftpToolWindowZhCN } from "./sftp-tool-window";
import { diagnosticsEn, diagnosticsZhCN } from "./diagnostics";
import { offlineBackupErrorsEn, offlineBackupErrorsZhCN } from "./offline-backup-errors";
import { applicationPreferenceErrorsEn, applicationPreferenceErrorsZhCN } from "./application-preference-errors";
import { en } from "./messages/en";
import { zhCN } from "./messages/zh-CN";

export type AppLocale = "zh-CN" | "en";
export type LocalePreference = AppLocale | "system";

export function resolveLocale(preference: LocalePreference, languages: readonly string[] = navigator.languages): AppLocale {
  if (preference !== "system") return preference;
  for (const language of languages) {
    if (/^zh(?:-|$)/i.test(language)) return "zh-CN";
    if (/^en(?:-|$)/i.test(language)) return "en";
  }
  return "en";
}

const messages = {
  "zh-CN": { ...zhCN, releases: releasesZhCN, secureWindow: secureWindowZhCN, sftpToolWindow: sftpToolWindowZhCN, diagnostics: diagnosticsZhCN, offlineBackupErrors: offlineBackupErrorsZhCN, applicationPreferenceErrors: applicationPreferenceErrorsZhCN },
  en: { ...en, releases: releasesEn, secureWindow: secureWindowEn, sftpToolWindow: sftpToolWindowEn, diagnostics: diagnosticsEn, offlineBackupErrors: offlineBackupErrorsEn, applicationPreferenceErrors: applicationPreferenceErrorsEn },
} as const;

export const i18n = createI18n({
  legacy: false,
  locale: resolveLocale("system"),
  fallbackLocale: "en",
  messages,
});
