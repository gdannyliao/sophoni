<script lang="ts">
  export let diff: string;
</script>

<div class="diff-viewer" data-testid="diff-viewer">
  {#each diff.split("\n") as line}
    {#if line.startsWith("+") && !line.startsWith("+++")}
      <div class="diff-line add">{line}</div>
    {:else if line.startsWith("-") && !line.startsWith("---")}
      <div class="diff-line del">{line}</div>
    {:else}
      <div class="diff-line ctx">{line}</div>
    {/if}
  {/each}
</div>

<style>
  .diff-viewer {
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.7;
  }
  .diff-line {
    padding: 1px var(--space-4);
    white-space: pre-wrap;
  }
  .diff-line.add {
    background: var(--add-bg);
    color: var(--success);
  }
  .diff-line.del {
    background: var(--del-bg);
    color: var(--danger);
  }
  .diff-line.ctx {
    color: var(--text-secondary);
  }
</style>
