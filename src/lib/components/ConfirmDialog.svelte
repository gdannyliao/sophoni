<script lang="ts">
  import type { CommandConfirmRequest } from "../types";

  export let request: CommandConfirmRequest;
  export let onResolve: (allowed: boolean) => void = () => {};
</script>

<div class="overlay" role="button" tabindex="0" on:click={() => onResolve(false)}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="dialog" role="dialog" aria-modal="true" data-testid="confirm-dialog" on:click|stopPropagation>
    <div class="dialog-header">
      <span class="warn-icon">⚠️</span>
      <span class="dialog-title">命令确认</span>
    </div>
    <div class="dialog-body">
      <p class="reason">{request.reason}</p>
      <pre class="command-text" data-testid="confirm-command">{request.command}</pre>
    </div>
    <div class="dialog-actions">
      <button class="btn deny-btn" data-testid="confirm-deny" on:click={() => onResolve(false)}>拒绝</button>
      <button class="btn btn-primary allow-btn" data-testid="confirm-allow" on:click={() => onResolve(true)}>允许执行</button>
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
    z-index: 200;
    border: 0;
    padding: 0;
    width: 100%;
    cursor: default;
    text-align: left;
    font: inherit;
    color: inherit;
  }
  .dialog {
    background: var(--bg-secondary);
    border: 1px solid var(--danger);
    border-radius: var(--radius-lg);
    min-width: 420px;
    max-width: 600px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.4);
  }
  .dialog-header {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-4) var(--space-6);
    border-bottom: 1px solid var(--border);
  }
  .warn-icon { font-size: 18px; }
  .dialog-title { font-weight: 600; }
  .dialog-body { padding: var(--space-4) var(--space-6); }
  .reason {
    margin: 0 0 var(--space-3) 0;
    color: var(--danger);
    font-size: 13px;
  }
  .command-text {
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3);
    font-family: var(--font-mono);
    font-size: 13px;
    white-space: pre-wrap;
    word-break: break-all;
    margin: 0;
  }
  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
    padding: var(--space-3) var(--space-6);
    border-top: 1px solid var(--border);
  }
  .deny-btn {
    color: var(--danger);
    border-color: var(--danger);
  }
</style>
