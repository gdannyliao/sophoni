<script lang="ts">
  import type { AgentEvent } from "../types";

  export let events: AgentEvent[] = [];
  export let summary = "输入任务后，Agent 会在这里展示步骤和结果。";
</script>

<main class="conversation" aria-label="任务会话流">
  <header class="topbar">
    <div>
      <h1>桌面 Agent 工作台</h1>
      <p>macOS MVP 基础骨架</p>
    </div>
  </header>

  <div class="messages">
    {#each events as event}
      <article class="event">
        <span>{event.kind}</span>
        <h3>{event.title}</h3>
        <p>{event.body}</p>
      </article>
    {/each}
    <article class="assistant">
      <h3>结果摘要</h3>
      <p>{summary}</p>
    </article>
  </div>

  <form class="composer">
    <input aria-label="任务输入" placeholder="让 Agent 搜索、修改或解释当前工作区..." />
    <button type="button">运行 mock 任务</button>
  </form>
</main>
