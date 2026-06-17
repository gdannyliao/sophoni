/**
 * 平台检测。用 @tauri-apps/plugin-os 的 platform() 运行时检测，
 * 比 import.meta.env 编译期变量更可靠（dev 模式下 env 注入不稳定）。
 *
 * 注意：platform() 是异步 IPC 调用，所以 isMobile 是 async。
 * main.ts 在 mount 前调用，确保分流正确。
 */

import { platform } from "@tauri-apps/plugin-os";

/** 是否移动端（Android / iOS）。异步，因为要调原生 IPC。 */
export async function isMobile(): Promise<boolean> {
  try {
    const p = await platform();
    return p === "android" || p === "ios";
  } catch {
    // IPC 失败（极端情况）按桌面处理
    return false;
  }
}
