<script lang="ts">
  import type { FileChange } from "../types";
  import DiffViewer from "./DiffViewer.svelte";

  export let fileChanges: FileChange[];
  export let onClose: () => void = () => {};

  let selectedIndex = 0;
  let viewMode: "diff" | "source" = "diff";

  $: selected = fileChanges[selectedIndex] ?? null;
</script>

<div class="review-view" data-testid="review-view">
  <header class="review-header">
    <span class="review-title">文件变更（{fileChanges.length}）</span>
    <button class="btn" on:click={onClose}>✕ 关闭</button>
  </header>

  <div class="review-body">
    <div class="file-list">
      {#each fileChanges as change, i}
        <div
          class="file-item"
          class:selected={i === selectedIndex}
          role="button"
          tabindex="0"
          on:click={() => (selectedIndex = i)}
          on:keydown={(e) => e.key === "Enter" && (selectedIndex = i)}
        >
          <span>📝</span>
          <span class="file-path">{change.path}</span>
          <span class="file-kind" class:created={change.kind === "created"}>
            {change.kind === "created" ? "新" : "改"}
          </span>
        </div>
      {/each}
    </div>

    <div class="file-detail">
      {#if selected}
        <div class="detail-header">
          <span class="detail-path">{selected.path}</span>
          <div class="view-toggle">
            <button class="toggle-btn" class:active={viewMode === "diff"} on:click={() => (viewMode = "diff")}>Diff</button>
            <button class="toggle-btn" class:active={viewMode === "source"} on:click={() => (viewMode = "source")}>原文件</button>
          </div>
        </div>
        <div class="detail-content">
          {#if viewMode === "diff"}
            <DiffViewer diff={selected.diff} />
          {:else}
            <pre class="source-view">{selected.diff.replace(/^[-+ ] /gm, "").replace(/^---.*$/m, "").replace(/^\+\+\+.*$/m, "").trim()}</pre>
          {/if}
        </div>
      {:else}
        <div class="empty">无文件变更</div>
      {/if}
    </div>
  </div>

  <footer class="review-footer">
    <button class="btn" on:click={() => navigator.clipboard?.writeText(fileChanges.map((c) => c.diff).join("\n\n"))}>复制 diff</button>
  </footer>
</div>

<style>
  .review-view {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--bg-primary);
  }
  .review-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--space-3) var(--space-6);
    border-bottom: 1px solid var(--border);
  }
  .review-title { font-weight: 600; }
  .review-body {
    flex: 1;
    display: grid;
    grid-template-columns: 240px 1fr;
    overflow: hidden;
  }
  .file-list {
    border-right: 1px solid var(--border);
    background: var(--bg-secondary);
    padding: var(--space-2) 0;
    overflow: auto;
  }
  .file-item {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-4);
    cursor: pointer;
  }
  .file-item.selected {
    background: rgba(31, 111, 235, 0.15);
    border-left: 2px solid var(--accent);
  }
  .file-path { font-family: var(--font-mono); font-size: 12px; }
  .file-kind {
    margin-left: auto;
    font-size: 10px;
    padding: 1px var(--space-1);
    border-radius: var(--radius-sm);
    background: rgba(31, 111, 235, 0.2);
    color: var(--accent);
  }
  .file-kind.created {
    background: rgba(63, 185, 80, 0.2);
    color: var(--success);
  }
  .file-detail { display: flex; flex-direction: column; overflow: hidden; }
  .detail-header {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-4);
    border-bottom: 1px solid var(--border);
    background: var(--bg-secondary);
  }
  .detail-path { font-family: var(--font-mono); font-size: 13px; font-weight: 600; }
  .view-toggle {
    margin-left: auto;
    display: flex;
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 2px;
  }
  .toggle-btn {
    border: 0;
    background: transparent;
    color: var(--text-secondary);
    padding: 3px var(--space-3);
    font-size: 11px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .toggle-btn.active {
    background: var(--accent-bg);
    color: white;
  }
  .detail-content { flex: 1; overflow: auto; padding: var(--space-3) 0; }
  .source-view {
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.7;
    color: var(--text-secondary);
    padding: 0 var(--space-4);
    white-space: pre-wrap;
  }
  .empty { display: flex; align-items: center; justify-content: center; height: 100%; color: var(--text-secondary); }
  .review-footer {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
    padding: var(--space-3) var(--space-6);
    border-top: 1px solid var(--border);
  }
</style>
