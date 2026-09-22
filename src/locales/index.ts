import { createI18n } from "vue-i18n";

import { releasesEn, releasesZhCN } from "./releases";
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
  "zh-CN": { ...zhCN, releases: releasesZhCN },
  en: { ...en, releases: releasesEn },
} as const;

export const i18n = createI18n({
  legacy: false,
  locale: resolveLocale("system"),
  fallbackLocale: "en",
  messages,
});
