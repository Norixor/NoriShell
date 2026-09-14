import { createI18n } from "vue-i18n";

import { en } from "./messages/en";
import { zhCN } from "./messages/zh-CN";

export type AppLocale = "zh-CN" | "en";

const messages = {
  "zh-CN": zhCN,
  en,
} as const;

export const i18n = createI18n({
  legacy: false,
  locale: "zh-CN",
  fallbackLocale: "en",
  messages,
});
