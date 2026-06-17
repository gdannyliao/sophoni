<script lang="ts">
  import { onMount } from "svelte";
  import { getConfigStatus, getSearchConfig, saveSearchConfig, getPairQrcode } from "../api";
  import type { PairQrCode } from "../api";
  import type { ConfigStatus, SearchConfig } from "../types";

  export let onClose: () => void = () => {};

  let status: ConfigStatus | null = null;
  let searchConfig: SearchConfig = { tavilyKey: null, googleKey: null, googleCx: null };
  let searchSaved = false;
  let pairQr: PairQrCode | null = null;
  let pairLoading = false;

  onMount(async () => {
    try {
      status = await getConfigStatus();
    } catch {
      status = { configured: false, provider: "(查询失败)", model: "(查询失败)" };
    }
    try {
      const sc = await getSearchConfig();
      searchConfig = sc ?? { tavilyKey: null, googleKey: null, googleCx: null };
    } catch {
      // 未配置时保持默认空值
    }
  });

  async function loadPairInfo() {
    pairLoading = true;
    try {
      pairQr = await getPairQrcode();
    } catch {
      pairQr = null;
    } finally {
      pairLoading = false;
    }
  }

  async function onSaveSearch() {
    try {
      await saveSearchConfig(searchConfig);
      searchSaved = true;
      setTimeout(() => (searchSaved = false), 2000);
    } catch {
      // 静默失败，避免阻塞 UI
    }
  }
</script>

<div class="settings-panel" role="dialog" aria-modal="true" aria-label="设置" data-testid="settings-panel">
  <div class="settings-header">
    <h2>设置</h2>
    <button class="btn icon-only" on:click={onClose}>✕</button>
  </div>
  <div class="settings-body">
    {#if status}
      <div class="settings-row">
        <span class="label">Provider</span>
        <span>{status.provider}</span>
      </div>
      <div class="settings-row">
        <span class="label">状态</span>
        <span class={status.configured ? "status-ok" : "status-err"}>
          {status.configured ? "已配置" : "未配置"}
        </span>
      </div>
      <div class="settings-row">
        <span class="label">模型</span>
        <span class="mono">{status.model}</span>
      </div>
      {#if !status.configured}
        <p class="hint">请在 <code>~/.config/sophoni/config.toml</code> 配置 Provider，参考 README。</p>
      {/if}
    {/if}


    <div class="settings-row" style="margin-top: 16px;">
      <span class="label">网络搜索</span>
    </div>
    <p class="hint">配置后 agent 可用 web_search 搜索网络、web_fetch 读取网页。至少配一个即可。</p>
    <div class="search-config" data-testid="search-config">
      <label class="search-field">
        <span class="field-label">Tavily API Key</span>
        <input type="password" data-testid="search-tavily-key" bind:value={searchConfig.tavilyKey} placeholder="tvly-..." />
      </label>
      <label class="search-field">
        <span class="field-label">Google API Key</span>
        <input type="password" data-testid="search-google-key" bind:value={searchConfig.googleKey} placeholder="（可选）" />
      </label>
      <label class="search-field">
        <span class="field-label">Google 搜索引擎 ID (CX)</span>
        <input type="text" data-testid="search-google-cx" bind:value={searchConfig.googleCx} placeholder="（可选，配 Google 时需要）" />
      </label>
      <button class="btn btn-primary" data-testid="search-save" on:click={onSaveSearch}>
        {searchSaved ? "已保存" : "保存搜索配置"}
      </button>
    </div>

    <div class="settings-row" style="margin-top: 16px;">
      <span class="label">手机连接</span>
      <button class="btn icon-only" data-testid="mobile-pair-toggle" on:click={loadPairInfo} disabled={pairLoading}>
        {pairLoading ? "..." : pairQr ? "刷新" : "显示"}
      </button>
    </div>
    {#if pairQr}
      <div class="pair-panel" data-testid="pair-panel">
        <div class="pair-qr" data-testid="pair-qr-svg">
          <!-- 二维码 SVG 内联渲染 -->
          {@html pairQr.svg}
        </div>
        <div class="pair-info">
          <div class="pair-line">
            <span class="label">地址</span>
            <code class="mono">{pairQr.ip}:{pairQr.port}</code>
          </div>
          <div class="pair-line">
            <span class="label">配对码</span>
            <code class="mono pair-code" data-testid="pair-code-display">{pairQr.code}</code>
          </div>
          <p class="pair-hint">手机和电脑需在同一 Wi-Fi。配对码一次性，配对成功后自动轮换。</p>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .settings-panel {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    min-width: 400px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.4);
  }
  .settings-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--space-4) var(--space-6);
    border-bottom: 1px solid var(--border);
  }
  .settings-header h2 { margin: 0; font-size: 16px; }
  .settings-body { padding: var(--space-4) var(--space-6); }
  .settings-row {
    display: flex;
    justify-content: space-between;
    padding: var(--space-2) 0;
  }
  .label { color: var(--text-secondary); font-size: 13px; }
  .mono { font-family: var(--font-mono); font-size: 13px; }
  .status-ok { color: var(--success); }
  .status-err { color: var(--danger); }
  .hint { font-size: 12px; color: var(--text-secondary); margin-top: var(--space-3); }
  code { font-family: var(--font-mono); }
  .icon-only { padding: var(--space-1) var(--space-2); }
  .search-config {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-2) 0;
  }
  .search-field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    font-size: 13px;
  }
  .field-label { color: var(--text-secondary); font-size: 12px; }
  .search-field input {
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--space-2) var(--space-3);
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .pair-panel {
    display: flex;
    gap: var(--space-4);
    padding: var(--space-3);
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    margin-top: var(--space-2);
  }
  .pair-qr {
    width: 140px;
    height: 140px;
    flex-shrink: 0;
  }
  .pair-qr :global(svg) {
    width: 100%;
    height: 100%;
  }
  .pair-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .pair-line {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .pair-code {
    font-size: 18px;
    font-weight: 600;
    color: var(--accent);
    letter-spacing: 2px;
  }
  .pair-hint {
    font-size: 11px;
    color: var(--text-secondary);
    margin-top: var(--space-1);
  }
</style>
