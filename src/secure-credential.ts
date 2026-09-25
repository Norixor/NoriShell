import { createApp } from "vue";
import { initializeSecureWindowAppearance } from "./secure-window";
import { i18n } from "./locales";
import SecureCredential from "./views/SecureCredential.vue";
import { revealWindowAfterMount } from "./window-first-show";
import "./styles/tokens.css";
import "./styles/base.css";

initializeSecureWindowAppearance();
createApp(SecureCredential).use(i18n).mount("#secure-app");
void revealWindowAfterMount();
