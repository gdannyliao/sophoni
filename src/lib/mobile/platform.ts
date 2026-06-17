/**
 * 平台检测。Tauri 2 在构建期通过 Vite 注入 import.meta.env.TAURI_ENV_PLATFORM。
 * 值为 "android" / "ios" 时是移动端，"macos"/"linux"/"windows" 是桌面端。
 *
 * 用编译期变量而非运行时 UA 嗅探——Tauri 确保它在打包时正确注入。
 */

/** 是否移动端（Android / iOS）。决定走 mobile-api 还是桌面 api。 */
export function isMobile(): boolean {
  const platform = import.meta.env.TAURI_ENV_PLATFORM;
  return platform === "android" || platform === "ios";
}

/** 是否桌面端。 */
export function isDesktop(): boolean {
  return !isMobile();
}
