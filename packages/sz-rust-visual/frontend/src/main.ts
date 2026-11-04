import { createApp } from "vue";
import { createPinia } from "pinia";
import { createI18n } from "vue-i18n";
import App from "./App.vue";
import zhCN from "./i18n/zh-CN.json";
import enUS from "./i18n/en-US.json";

const i18n = createI18n({
    legacy: false,
    locale: "zh-CN",
    fallbackLocale: "en-US",
    messages: { "zh-CN": zhCN, "en-US": enUS },
});

const app = createApp(App);
app.use(createPinia());
app.use(i18n);
app.mount("#app");