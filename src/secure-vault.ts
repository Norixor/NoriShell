import { createApp } from "vue";
import { initializeSecureWindowAppearance } from "./secure-window";
import { i18n } from "./locales";
import SecureVault from "./views/SecureVault.vue";
import "./styles/tokens.css";
import "./styles/base.css";
initializeSecureWindowAppearance();
createApp(SecureVault).use(i18n).mount("#secure-app");
