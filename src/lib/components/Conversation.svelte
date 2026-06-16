<script lang="ts">
  import type { AgentEvent } from "../types";
  import MessageBubble from "./MessageBubble.svelte";
  import ThoughtLine from "./ThoughtLine.svelte";
  import CommandCard from "./CommandCard.svelte";
  import ChangeNotice from "./ChangeNotice.svelte";

  export let events: AgentEvent[] = [];
  export let summary = "";
  export let streamingText = "";
  export let prompt = "";
  export let running = false;
  export let workspacePath = "";
  export let title = "";
  export let changeCount = 0;
  export let onRun: (prompt: string) => void = () => {};
  export let onCancel: () => void = () => {};
  export let onReview: () => void = () => {};

  // 消息区容器引用，用于 token 到达时自动滚动到底部。
  let messagesEl: HTMLDivElement | null = null;

  // 处理事件流：用户消息、命令卡片、变更通知
  type DisplayItem =
    | { type: "user"; content: string }
    | { type: "thought"; title: string }
    | { type: "command"; id: string; command: string; exitCode: number | null; stdout: string; stderr: string }
    | { type: "change"; path: string; kind: "created" | "modified" | "deleted" }
    | { type: "error"; body: string };

  $: items = processEvents(events);

  function processEvents(events: AgentEvent[]): DisplayItem[] {
    const items: DisplayItem[] = [];
    const commandMap = new Map<string, DisplayItem & { type: "command" }>();

    for (const ev of events) {
      // tool_call: run_command → 创建命令卡片
      if (ev.kind === "tool_call" && ev.title.startsWith("run_command:")) {
        const command = ev.title.slice("run_command: ".length);
        const item: DisplayItem & { type: "command" } = {
          type: "command",
          id: ev.toolCallId ?? ev.title,
          command,
          exitCode: null,
          stdout: "",
          stderr: "",
        };
        if (ev.toolCallId) commandMap.set(ev.toolCallId, item);
        items.push(item);
      }
      // tool_call: edit_file / write_file → 变更通知
      else if (ev.kind === "tool_call" && (ev.title.startsWith("edit_file:") || ev.title.startsWith("write_file:"))) {
        const path = ev.title.split(":")[1]?.trim().split(" ")[0] ?? "";
        const kind = ev.title.startsWith("write_file:") ? "created" : "modified";
        items.push({ type: "change", path, kind });
      }
      // tool_result: 填充对应命令卡片
      else if (ev.kind === "tool_result" && ev.toolCallId && commandMap.has(ev.toolCallId)) {
        const cmd = commandMap.get(ev.toolCallId)!;
        const isExit = ev.body.startsWith("exit code: ");
        if (isExit) {
          cmd.exitCode = parseInt(ev.body.match(/exit code: (\d+)/)?.[1] ?? "-1");
          const stdoutMatch = ev.body.match(/--- stdout ---\n([\s\S]*?)(\n--- stderr ---|$)/);
          const stderrMatch = ev.body.match(/--- stderr ---\n([\s\S]*)/);
          cmd.stdout = stdoutMatch?.[1]?.trim() ?? "";
          cmd.stderr = stderrMatch?.[1]?.trim() ?? "";
        }
      }
      // thought
      else if (ev.kind === "thought") {
        items.push({ type: "thought", title: ev.title });
      }
      // error
      else if (ev.kind === "error") {
        items.push({ type: "error", body: ev.body });
      }
    }
    return items;
  }

  // token 到达或事件变化时，自动滚到底部，保证流式输出始终可见。
  // streamingText 经 App.svelte 的 rAF 节流后每帧最多变一次，滚动开销可控。
  $: if (streamingText && messagesEl) {
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }
</script>

<main class="conversation" data-testid="conversation">
  <header class="topbar">
    <div class="topbar-left">
      <div>
        <div class="task-title">{title}</div>
        <div class="workspace-path">{workspacePath}</div>
      </div>
    </div>
    <button class="btn review-btn" on:click={onReview} disabled={changeCount === 0}>
      📝 查看修改
      {#if changeCount > 0}
        <span class="badge">{changeCount}</span>
      {/if}
    </button>
  </header>

  <div class="messages" bind:this={messagesEl} aria-label="任务会话流">
    {#each items as item}
      <div class="agent-event" data-testid="agent-event">
        {#if item.type === "user"}
          <MessageBubble content={item.content} />
        {:else if item.type === "thought"}
          <ThoughtLine title={item.title} />
        {:else if item.type === "command"}
          <CommandCard command={item.command} exitCode={item.exitCode} stdout={item.stdout} stderr={item.stderr} />
        {:else if item.type === "change"}
          <ChangeNotice path={item.path} kind={item.kind} />
        {:else if item.type === "error"}
          <div class="error-card">{item.body}</div>
        {/if}
      </div>
    {/each}
    {#if streamingText}
      <div class="streaming-bubble" data-testid="streaming-bubble" aria-live="polite">
        <span class="streaming-text">{streamingText}</span>
        <span class="streaming-cursor" aria-hidden="true">▍</span>
      </div>
    {/if}
    {#if summary}
      <div class="summary-card">
        <div class="summary-label">结果摘要</div>
        <div>{summary}</div>
      </div>
    {/if}
  </div>

  <form class="composer" on:submit|preventDefault={() => onRun(prompt)}>
    <input data-testid="task-input" aria-label="任务输入" placeholder="让 Agent 读取、修改工作区文件..." bind:value={prompt} />
    <button data-testid="run-button" class="btn btn-primary" type="submit" disabled={running}>
      {running ? "运行中..." : "发送"}
    </button>
    {#if running}
      <button type="button" class="btn cancel-btn" on:click={onCancel}>取消</button>
    {/if}
  </form>
</main>

<style>
  .conversation {
    display: grid;
    grid-template-rows: auto 1fr auto;
    min-width: 0;
    height: 100vh;
  }
  .topbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--space-3) var(--space-6);
    border-bottom: 1px solid var(--border);
    background: var(--bg-secondary);
  }
  .topbar-left { display: flex; align-items: center; gap: var(--space-3); }
  .task-title { font-weight: 600; font-size: 15px; }
  .workspace-path { font-size: 11px; color: var(--text-secondary); font-family: var(--font-mono); }
  .review-btn { display: flex; align-items: center; gap: var(--space-2); }
  .badge {
    background: var(--accent-bg);
    color: white;
    border-radius: 10px;
    padding: 1px 7px;
    font-size: 11px;
  }
  .messages {
    padding: var(--space-6);
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .error-card {
    background: rgba(248, 81, 73, 0.1);
    border: 1px solid var(--danger);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    color: var(--danger);
    font-size: 13px;
  }
  .summary-card {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
  }
  .summary-label {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--text-secondary);
    margin-bottom: var(--space-2);
  }
  .streaming-bubble {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.5;
  }
  .streaming-text { font-size: 14px; }
  .streaming-cursor {
    display: inline-block;
    color: var(--text-secondary);
    animation: blink 1s steps(2, start) infinite;
  }
  @keyframes blink {
    to { visibility: hidden; }
  }
  .composer {
    display: flex;
    gap: var(--space-2);
    padding: var(--space-4) var(--space-6);
    border-top: 1px solid var(--border);
    background: var(--bg-secondary);
  }
  .composer input {
    flex: 1;
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-2) var(--space-3);
    color: var(--text-primary);
  }
  .composer input::placeholder { color: var(--text-secondary); }
  .cancel-btn { color: var(--danger); border-color: var(--danger); }
</style>
