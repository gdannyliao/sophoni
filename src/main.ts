import { mount } from "svelte";
import "./app.css";
import { isMobile } from "./lib/mobile/platform";
import App from "./App.svelte";
import MobileApp from "./MobileApp.svelte";

// 平台分流：移动端（Android/iOS）走 MobileApp，桌面端走 App。
// 用 @tauri-apps/plugin-os 运行时检测（异步 IPC），比编译期 env 变量可靠。
async function bootstrap() {
  const mobile = await isMobile();
  const Root = mobile ? MobileApp : App;
  mount(Root, {
    target: document.getElementById("app")!,
  });
}

bootstrap();
