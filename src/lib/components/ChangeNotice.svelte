<script lang="ts">
  export let path: string;
  export let kind: "created" | "modified" | "deleted";
</script>

<div class="change-notice" data-testid="change-notice">
  <span class="icon">📝</span>
  <span class="path">{path}</span>
  <span class="kind-badge" class:created={kind === "created"} class:modified={kind === "modified"}>
    {kind === "created" ? "新增" : kind === "modified" ? "已修改" : "已删除"}
  </span>
</div>

<style>
  .change-notice {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    background: rgba(35, 134, 54, 0.08);
    border: 1px solid rgba(63, 185, 80, 0.3);
    border-radius: var(--radius-md);
    padding: var(--space-2) var(--space-4);
  }
  .path {
    font-family: var(--font-mono);
    font-size: 13px;
  }
  .kind-badge {
    margin-left: auto;
    font-size: 11px;
    padding: 2px var(--space-2);
    border-radius: var(--radius-sm);
  }
  .kind-badge.created {
    background: rgba(63, 185, 80, 0.2);
    color: var(--success);
  }
  .kind-badge.modified {
    background: rgba(31, 111, 235, 0.2);
    color: var(--accent);
  }
</style>
