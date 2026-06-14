<script lang="ts">
  import type { FileChange } from "../types";

  export let fileChanges: FileChange[] = [];
</script>

<aside class="context" aria-label="上下文与产物">
  <section>
    <h2>文件与变更</h2>
    {#if fileChanges.length === 0}
      <p class="muted">暂无文件变更</p>
    {:else}
      {#each fileChanges as change}
        <article class="change">
          <h3>{change.path}</h3>
          <pre>{change.diff}</pre>
        </article>
      {/each}
    {/if}
  </section>
  <section>
    <h2>工具日志</h2>
    <p class="muted">mock 工具事件会显示在中栏。</p>
  </section>
  <section>
    <h2>风险提示</h2>
    <p class="muted">高风险命令将在后续任务中接入确认流。</p>
  </section>
</aside>
