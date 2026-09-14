import { createApp } from "vue";

import { initializeSecureWindowAppearance } from "./secure-window";

import { i18n } from "./locales";
import SecurePluginPermission from "./views/SecurePluginPermission.vue";
import "./styles/tokens.css";
import "./styles/base.css";

initializeSecureWindowAppearance();

createApp(SecurePluginPermission).use(i18n).mount("#secure-app");
