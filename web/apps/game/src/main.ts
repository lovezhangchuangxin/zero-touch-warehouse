import "./styles/main.css";
import { createApp } from "vue";
import App from "./App.vue";
import { installScrollGuards } from "./pageScroll";
import { bootstrap } from "./store";

createApp(App).mount("#app");

installScrollGuards();

void bootstrap();
