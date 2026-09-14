import { createApp } from "vue";
import { createPinia } from "pinia";

import App from "./App.vue";
import { i18n } from "./locales";
import { router } from "./router";
import "./styles/tokens.css";
import "./styles/base.css";

createApp(App).use(createPinia()).use(i18n).use(router).mount("#app");

