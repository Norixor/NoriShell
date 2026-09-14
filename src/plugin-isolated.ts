import { createApp } from "vue";
import { initializeSecureWindowAppearance } from "./secure-window";
import { i18n } from "./locales";
import PluginIsolatedWrapper from "./views/PluginIsolatedWrapper.vue";
import "./styles/tokens.css";
import "./styles/base.css";

initializeSecureWindowAppearance();
createApp(PluginIsolatedWrapper).use(i18n).mount("#isolated-root");
