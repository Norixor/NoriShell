import { createApp } from "vue";

import { initializeSecureWindowAppearance } from "./secure-window";

import { i18n } from "./locales";
import SecurePluginHostApproval from "./views/SecurePluginHostApproval.vue";
import "./styles/tokens.css";
import "./styles/base.css";

initializeSecureWindowAppearance();

createApp(SecurePluginHostApproval).use(i18n).mount("#secure-app");
