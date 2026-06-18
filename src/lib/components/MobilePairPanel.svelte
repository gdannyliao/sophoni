<script lang="ts">
  import { getPairQrcode } from "../api";
  import type { PairQrCode } from "../api";

  export let onClose: () => void = () => {};

  let pairQr: PairQrCode | null = null;
  let loading = false;

  async function load() {
    loading = true;
    try {
      pairQr = await getPairQrcode();
    } catch {
      pairQr = null;
    } finally {
      loading = false;
    }
  }

  // 打开即加载
  load();
</script>

<div class="overlay" on:click={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="panel" role="dialog" aria-modal="true" aria-label="手机连接" on:click|stopPropagation data-testid="mobile-pair-panel">
    <div class="panel-header">
      <h2>📱 手机连接</h2>
      <button class="btn icon-only" on:click={onClose}>✕</button>
    </div>
    <div class="panel-body">
      <button class="btn btn-primary" data-testid="mobile-pair-refresh" on:click={load} disabled={loading}>
        {loading ? "加载中..." : pairQr ? "刷新二维码" : "显示二维码"}
      </button>

      {#if pairQr}
        <div class="pair-content" data-testid="pair-panel">
          <div class="pair-qr" data-testid="pair-qr-svg">
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
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    border: 0;
    padding: 0;
  }
  .panel {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    min-width: 400px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.4);
  }
  .panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--space-4) var(--space-6);
    border-bottom: 1px solid var(--border);
  }
  .panel-header h2 { margin: 0; font-size: 16px; }
  .panel-body { padding: var(--space-4) var(--space-6); }
  .btn-primary { margin-bottom: var(--space-4); }
  .pair-content {
    display: flex;
    gap: var(--space-4);
    padding: var(--space-3);
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
  }
  .pair-qr { width: 140px; height: 140px; flex-shrink: 0; }
  .pair-qr :global(svg) { width: 100%; height: 100%; }
  .pair-info { flex: 1; display: flex; flex-direction: column; gap: var(--space-2); }
  .pair-line { display: flex; flex-direction: column; gap: 2px; }
  .label { font-size: 12px; color: var(--text-secondary); }
  .mono { font-family: var(--font-mono); font-size: 13px; }
  .pair-code { font-size: 18px; font-weight: 600; color: var(--accent); letter-spacing: 2px; }
  .pair-hint { font-size: 11px; color: var(--text-secondary); margin-top: var(--space-1); }
  .icon-only { padding: var(--space-1) var(--space-2); }
</style>
