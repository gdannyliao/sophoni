/// <reference types="svelte" />
/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Tauri 注入的目标平台：android / ios / macos / linux / windows。 */
  readonly TAURI_ENV_PLATFORM?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
