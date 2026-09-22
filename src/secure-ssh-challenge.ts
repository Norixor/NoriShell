import { createApp } from "vue";
import { initializeSecureWindowAppearance } from "./secure-window";
import { i18n } from "./locales";
import SecureSshChallenge from "./views/SecureSshChallenge.vue";
import "./styles/tokens.css";
import "./styles/base.css";
initializeSecureWindowAppearance();
createApp(SecureSshChallenge).use(i18n).mount("#secure-app");
