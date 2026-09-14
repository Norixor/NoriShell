import { createApp } from "vue";

import { initializeSecureWindowAppearance } from "./secure-window";

import { i18n } from "./locales";
import SecurePluginTerminalInput from "./views/SecurePluginTerminalInput.vue";
import "./styles/tokens.css";
import "./styles/base.css";

initializeSecureWindowAppearance();

createApp(SecurePluginTerminalInput).use(i18n).mount("#secure-app");
