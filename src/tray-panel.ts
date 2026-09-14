import { createApp } from "vue";
import { i18n } from "./locales";
import { initializeAuxiliaryThemeAppearance } from "./app-theme-projection";
import { initializeSecureWindowAppearance } from "./secure-window";
import TrayPanel from "./views/TrayPanel.vue";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/tray-panel.css";

// Initialize shared appearance only; never load the main-window router, terminal store, or plugin runtime.
initializeSecureWindowAppearance();
initializeAuxiliaryThemeAppearance();
createApp(TrayPanel).use(i18n).mount("#tray-app");
