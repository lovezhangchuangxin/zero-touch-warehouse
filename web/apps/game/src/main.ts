import { createApp } from "vue";
import App from "./App.vue";
import { bootstrap } from "./store";

createApp(App).mount("#app");

void bootstrap();
