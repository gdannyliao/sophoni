import { mount } from "svelte";
import "./app.css";
import { isMobile } from "./lib/mobile/platform";
import App from "./App.svelte";
import MobileApp from "./MobileApp.svelte";

// 平台分流：移动端（Android/iOS）走 MobileApp（配对 + mobile-api），
// 桌面端走 App（Tauri IPC）。Tauri 构建期注入 import.meta.env.TAURI_ENV_PLATFORM。
const Root = isMobile() ? MobileApp : App;

const app = mount(Root, {
  target: document.getElementById("app")!,
});

export default app;
