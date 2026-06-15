<script lang="ts">
  import type { AgentEvent } from "../types";

  export let events: AgentEvent[] = [];
  export let summary = "输入任务后，Agent 会在这里展示步骤和结果。";
  export let prompt = "";
  export let running = false;
  export let onRun: (prompt: string) => void = () => {};
  export let onCancel: () => void = () => {};
</script>

<main class="conversation" aria-label="任务会话流" data-testid="conversation">
  <header class="topbar">
    <div>
      <h1>桌面 Agent 工作台</h1>
      <p>macOS MVP · GLM 真连接入</p>
    </div>
  </header>

  <div class="messages">
    {#each events as event}
      <article class="event" data-kind={event.kind} data-testid="agent-event">
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

  <form class="composer" on:submit|preventDefault={() => onRun(prompt)}>
    <input data-testid="task-input" aria-label="任务输入" placeholder="让 Agent 读取、修改工作区文件..." bind:value={prompt} />
    <button data-testid="run-button" type="submit" disabled={running}>{running ? "运行中..." : "运行任务"}</button>
    {#if running}
      <button type="button" class="cancel" on:click={onCancel}>取消</button>
    {/if}
  </form>
</main>
