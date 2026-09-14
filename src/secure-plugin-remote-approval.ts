import { createApp } from "vue";

import { initializeSecureWindowAppearance } from "./secure-window";

import { i18n } from "./locales";
import SecurePluginRemoteApproval from "./views/SecurePluginRemoteApproval.vue";
import "./styles/tokens.css";
import "./styles/base.css";

initializeSecureWindowAppearance();

createApp(SecurePluginRemoteApproval).use(i18n).mount("#secure-app");
