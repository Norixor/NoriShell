import { createApp } from "vue";
import { initializeSecureWindowAppearance } from "./secure-window";
import { i18n } from "./locales";
import SecureDesktop from "./views/SecureDesktop.vue";
import "./styles/tokens.css";
import "./styles/base.css";
initializeSecureWindowAppearance();
createApp(SecureDesktop).use(i18n).mount("#secure-app");
