<script lang="ts">
  export let command: string;
  export let exitCode: number | null = null; // null = 运行中
  export let stdout = "";
  export let stderr = "";

  let expanded = false;

  $: isSuccess = exitCode !== null && exitCode === 0;
  $: isRunning = exitCode === null;
</script>

<div class="command-card" data-testid="command-card">
  <div
    class="command-header"
    role="button"
    tabindex="0"
    on:click={() => (expanded = !expanded)}
    on:keydown={(e) => e.key === "Enter" && (expanded = !expanded)}
  >
    <span class="status-icon" class:success={isSuccess} class:error={!isSuccess && !isRunning}>
      {#if isRunning}⏳{:else if isSuccess}✓{:else}✗{/if}
    </span>
    <span class="command-name">{command}</span>
    <span class="exit-code" class:success={isSuccess} class:error={!isSuccess && !isRunning}>
      {#if isRunning}运行中...{:else}exit {exitCode}{/if}
    </span>
    {#if !isRunning}
      <span class="expand-arrow">{expanded ? "▴" : "▾"}</span>
    {/if}
  </div>
  {#if expanded && !isRunning}
    <div class="command-output">
      {#if stdout}
        <div class="output-section">
          <div class="output-label">stdout</div>
          <pre>{stdout}</pre>
        </div>
      {/if}
      {#if stderr}
        <div class="output-section">
          <div class="output-label">stderr</div>
          <pre class="error-output">{stderr}</pre>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .command-card {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .command-header {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-4);
    background: var(--bg-tertiary);
    cursor: pointer;
    user-select: none;
  }
  .status-icon { font-family: var(--font-mono); }
  .status-icon.success { color: var(--success); }
  .status-icon.error { color: var(--danger); }
  .command-name {
    font-family: var(--font-mono);
    font-size: 13px;
  }
  .exit-code {
    margin-left: auto;
    font-size: 12px;
    font-family: var(--font-mono);
  }
  .exit-code.success { color: var(--success); }
  .exit-code.error { color: var(--danger); }
  .expand-arrow { color: var(--text-secondary); font-size: 11px; }
  .command-output { padding: var(--space-2) var(--space-4); }
  .output-section { margin-bottom: var(--space-2); }
  .output-label {
    font-size: 11px;
    color: var(--text-secondary);
    text-transform: uppercase;
    margin-bottom: var(--space-1);
  }
  pre {
    margin: 0;
    white-space: pre-wrap;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.6;
    color: var(--text-primary);
  }
  .error-output { color: var(--danger); }
</style>
