<script lang="ts">
  export let title: string;
  export let result = "";
  export let isError = false;
  export let pending = false;

  let expanded = false;
</script>

<div class="tool-card" data-testid="tool-card">
  <div
    class="tool-header"
    role="button"
    tabindex="0"
    on:click={() => !pending && (expanded = !expanded)}
    on:keydown={(e) => e.key === "Enter" && !pending && (expanded = !expanded)}
  >
    <span
      class="status-icon"
      class:success={!pending && !isError}
      class:error={isError}
    >
      {#if pending}⏳{:else if isError}✗{:else}✓{/if}
    </span>
    <span class="tool-title">{title}</span>
    {#if pending}
      <span class="status-text">运行中...</span>
    {:else}
      <span class="expand-arrow">{expanded ? "▴" : "▾"}</span>
    {/if}
  </div>
  {#if expanded && !pending}
    <div class="tool-output">
      <pre class:error={isError}>{result}</pre>
    </div>
  {/if}
</div>

<style>
  .tool-card {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .tool-header {
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
  .tool-title {
    font-family: var(--font-mono);
    font-size: 13px;
  }
  .status-text {
    margin-left: auto;
    font-size: 12px;
    font-family: var(--font-mono);
    color: var(--text-secondary);
  }
  .expand-arrow {
    margin-left: auto;
    color: var(--text-secondary);
    font-size: 11px;
  }
  .tool-output {
    padding: var(--space-2) var(--space-4);
    max-height: 400px;
    overflow: auto;
  }
  pre {
    margin: 0;
    white-space: pre-wrap;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.6;
    color: var(--text-primary);
  }
  pre.error { color: var(--danger); }
</style>
