<script lang="ts">
  import { pair } from "../mobile/mobile-api";
  import { parsePairUrl, saveConnection } from "../mobile/connection";

  export let onPaired: () => void = () => {};

  let baseUrl = "";
  let code = "";
  let error = "";
  let pairing = false;

  async function handlePair() {
    error = "";
    if (!baseUrl.trim() || !code.trim()) {
      error = "请填写桌面端地址和配对码";
      return;
    }

    // 兼容用户只输 IP:端口（补上 http://）
    let url = baseUrl.trim();
    if (!url.startsWith("http://") && !url.startsWith("https://")) {
      url = `http://${url}`;
    }

    pairing = true;
    try {
      const result = await pair(url, code.trim());
      saveConnection({ baseUrl: url, token: result.token });
      onPaired();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      pairing = false;
    }
  }

  // 支持粘贴二维码内容直接解析（降级：用户从别处复制 sophoni://pair?... 链接）
  function handlePasteLink() {
    try {
      const parsed = parsePairUrl(baseUrl.trim());
      if (parsed) {
        baseUrl = parsed.baseUrl;
        code = parsed.code;
        error = "";
      }
    } catch {
      // 不是链接格式，忽略
    }
  }
</script>

<div class="pairing" data-testid="pairing-view">
  <div class="logo">◈</div>
  <h1>连接桌面端</h1>
  <p class="subtitle">
    在桌面端打开「手机连接」面板，查看 IP 地址和配对码。
  </p>

  <form class="pair-form" on:submit|preventDefault={handlePair}>
    <label>
      <span class="label-text">桌面端地址</span>
      <input
        data-testid="pair-baseurl"
        type="text"
        placeholder="192.168.1.5:43210"
        bind:value={baseUrl}
        on:blur={handlePasteLink}
      />
    </label>

    <label>
      <span class="label-text">配对码（6 位数字）</span>
      <input
        data-testid="pair-code"
        type="text"
        inputmode="numeric"
        maxlength="6"
        placeholder="482910"
        bind:value={code}
      />
    </label>

    {#if error}
      <div class="error-msg" data-testid="pair-error">{error}</div>
    {/if}

    <button
      class="btn btn-primary"
      data-testid="pair-submit"
      type="submit"
      disabled={pairing}
    >
      {pairing ? "连接中..." : "连接"}
    </button>
  </form>

  <p class="hint">
    提示：确保手机和电脑在同一局域网（同一 Wi-Fi）。
  </p>
</div>

<style>
  .pairing {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    padding: var(--space-6);
  }
  .logo {
    font-size: 42px;
    color: var(--accent);
    margin-bottom: var(--space-2);
  }
  h1 {
    font-size: 22px;
    font-weight: 700;
    margin: 0 0 var(--space-2) 0;
  }
  .subtitle {
    font-size: 13px;
    color: var(--text-secondary);
    text-align: center;
    margin: 0 0 var(--space-6) 0;
    max-width: 300px;
  }
  .pair-form {
    width: 100%;
    max-width: 320px;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .label-text {
    font-size: 12px;
    color: var(--text-secondary);
  }
  input {
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    color: var(--text-primary);
    font-size: 15px;
    width: 100%;
    box-sizing: border-box;
  }
  input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .error-msg {
    color: var(--danger);
    font-size: 13px;
    text-align: center;
  }
  .btn-primary {
    width: 100%;
    padding: var(--space-3);
    font-size: 15px;
  }
  .hint {
    font-size: 11px;
    color: var(--text-secondary);
    margin-top: var(--space-6);
    text-align: center;
  }
</style>
