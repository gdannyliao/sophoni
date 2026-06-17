<script lang="ts">
  import { marked } from "marked";
  import type { AgentEvent } from "../types";
  import MessageBubble from "./MessageBubble.svelte";
  import ThoughtLine from "./ThoughtLine.svelte";
  import CommandCard from "./CommandCard.svelte";
  import ChangeNotice from "./ChangeNotice.svelte";
  import ToolCallCard from "./ToolCallCard.svelte";

  export let events: AgentEvent[] = [];
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

  // 中间过程项：一轮对话里 summary 之前的可折叠内容
  type ProcessItem =
    | { type: "thought"; title: string }
    | { type: "command"; id: string; command: string; exitCode: number | null; stdout: string; stderr: string }
    | { type: "change"; path: string; kind: "created" | "modified" | "deleted" }
    | { type: "round_timing"; title: string; body: string }
    | { type: "tool_read"; id: string; title: string; result: string; isError: boolean; pending: boolean };

  // 一轮对话：用户消息 + 中间过程 + 结果（summary 或 error）
  type Turn = {
    userContent: string;
    process: ProcessItem[];
    summary: string | null;
    error: string | null;
  };

  // events → turns：按 user 事件切分成多轮，每轮含中间过程和结果
  $: turns = processTurns(events);

  function processTurns(events: AgentEvent[]): Turn[] {
    const turns: Turn[] = [];
    const commandMap = new Map<string, ProcessItem & { type: "command" }>();
    const toolReadMap = new Map<string, ProcessItem & { type: "tool_read" }>();
    let current: Turn | null = null;

    const ensureTurn = (): Turn => {
      if (current === null) {
        current = { userContent: "", process: [], summary: null, error: null };
        turns.push(current);
      }
      return current;
    };

    for (const ev of events) {
      // user：开启一个新轮次
      if (ev.kind === "user") {
        current = { userContent: ev.body, process: [], summary: null, error: null };
        turns.push(current);
        continue;
      }
      // 以下事件归属当前轮；若没有前置 user（如历史脏数据），丢弃
      if (current === null) continue;

      // tool_call: run_command → 创建命令卡片
      if (ev.kind === "tool_call" && ev.title.startsWith("run_command:")) {
        const command = ev.title.slice("run_command: ".length);
        const item: ProcessItem & { type: "command" } = {
          type: "command",
          id: ev.toolCallId ?? ev.title,
          command,
          exitCode: null,
          stdout: "",
          stderr: "",
        };
        if (ev.toolCallId) commandMap.set(ev.toolCallId, item);
        current.process.push(item);
      }
      // tool_call: edit_file / write_file → 变更通知
      else if (ev.kind === "tool_call" && (ev.title.startsWith("edit_file:") || ev.title.startsWith("write_file:"))) {
        const path = ev.title.split(":")[1]?.trim().split(" ")[0] ?? "";
        const kind = ev.title.startsWith("write_file:") ? "created" : "modified";
        current.process.push({ type: "change", path, kind });
      }
      // tool_call: 其余只读工具（read_file/list_files/grep/验收相关）→ 统一工具卡片
      else if (ev.kind === "tool_call") {
        const item: ProcessItem & { type: "tool_read" } = {
          type: "tool_read",
          id: ev.toolCallId ?? ev.title,
          title: ev.title,
          result: "",
          isError: false,
          pending: true,
        };
        if (ev.toolCallId) toolReadMap.set(ev.toolCallId, item);
        current.process.push(item);
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
      // tool_result: 填充只读工具卡片（成功是原始 content，失败是 "失败: <msg>"）
      else if (ev.kind === "tool_result" && ev.toolCallId && toolReadMap.has(ev.toolCallId)) {
        const item = toolReadMap.get(ev.toolCallId)!;
        if (ev.body.startsWith("失败: ")) {
          item.isError = true;
          item.result = ev.body.slice("失败: ".length);
        } else {
          item.result = ev.body;
        }
        item.pending = false;
      }
      // thought
      else if (ev.kind === "thought") {
        current.process.push({ type: "thought", title: ev.body });
      }
      // summary：每轮最终答案（取最后一个）
      else if (ev.kind === "summary") {
        current.summary = ev.body;
      }
      // round_timing：轮次耗时徽章
      else if (ev.kind === "round_timing") {
        current.process.push({ type: "round_timing", title: ev.title, body: ev.body });
      }
      // error
      else if (ev.kind === "error") {
        current.error = ev.body;
      }
    }
    return turns;
  }

  // 折叠状态：按 turn 索引记录是否收起。summary 到达后默认收起；进行中/出错默认展开。
  // events 引用变化（会话切换/新会话）时整体重置，避免索引错位。
  let collapsed: Record<number, boolean> = {};
  $: events, (collapsed = {});
  $: collapsed = syncCollapsed(turns, collapsed);

  function syncCollapsed(turns: Turn[], prev: Record<number, boolean>): Record<number, boolean> {
    const next = { ...prev };
    for (let i = 0; i < turns.length; i++) {
      if (!(i in next)) {
        // 新索引：有 summary 默认收起，否则（进行中/出错）展开
        next[i] = turns[i].summary !== null;
      }
    }
    return next;
  }

  function toggle(i: number) {
    collapsed = { ...collapsed, [i]: !collapsed[i] };
  }

  function renderMarkdown(text: string): string {
    return marked.parse(text, { breaks: true, async: false }) as string;
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
        <div class="task-title" title={title}>{title.length > 20 ? title.slice(0, 20) + "…" : title}</div>
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
    {#each turns as turn, i}
      <div class="turn" data-testid="turn">
        <!-- 用户消息气泡（始终可见） -->
        <MessageBubble content={turn.userContent} />

        <!-- 中间过程：进行中/出错/已展开时显示 -->
        {#if !collapsed[i] && turn.process.length > 0}
          <div class="turn-process" data-testid="turn-process">
            {#each turn.process as item}
              <div class="agent-event" data-testid="agent-event">
                {#if item.type === "thought"}
                  <ThoughtLine title={item.title} />
                {:else if item.type === "round_timing"}
                  <div class="round-timing" data-testid="round-timing">⏱ {item.title} · {item.body}</div>
                {:else if item.type === "command"}
                  <CommandCard command={item.command} exitCode={item.exitCode} stdout={item.stdout} stderr={item.stderr} />
                {:else if item.type === "change"}
                  <ChangeNotice path={item.path} kind={item.kind} />
                {:else if item.type === "tool_read"}
                  <ToolCallCard title={item.title} result={item.result} isError={item.isError} pending={item.pending} />
                {/if}
              </div>
            {/each}
          </div>
        {/if}

        <!-- 折叠控件：仅当有 summary 且中间过程非空时显示 -->
        {#if turn.summary !== null && turn.process.length > 0}
          <button
            type="button"
            class="collapse-toggle"
            data-testid="collapse-toggle"
            on:click={() => toggle(i)}
          >
            {collapsed[i]
              ? `▸ 已执行 ${turn.process.length} 步（展开）`
              : `▾ 已执行 ${turn.process.length} 步（收起）`}
          </button>
        {/if}

        <!-- 结果摘要卡片（始终可见） -->
        {#if turn.summary !== null}
          <div class="summary-card" data-testid="summary-card">
            <div class="summary-label">结果摘要</div>
            <div class="markdown-body">{@html renderMarkdown(turn.summary)}</div>
          </div>
        {/if}

        <!-- 错误（始终可见，不折叠） -->
        {#if turn.error !== null}
          <div class="error-card">{turn.error}</div>
        {/if}
      </div>
    {/each}
    {#if streamingText}
      <div class="streaming-bubble" data-testid="streaming-bubble" aria-live="polite">
        <span class="markdown-body">{@html renderMarkdown(streamingText)}</span>
        <span class="streaming-cursor" aria-hidden="true">▍</span>
      </div>
    {/if}
  </div>

  <form class="composer" on:submit|preventDefault={() => { onRun(prompt); prompt = ""; }}>
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
  .turn {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .turn-process {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .collapse-toggle {
    align-self: flex-start;
    background: none;
    border: 0;
    padding: var(--space-1) var(--space-2);
    color: var(--text-secondary);
    font-size: 12px;
    cursor: pointer;
    border-radius: var(--radius-sm);
  }
  .collapse-toggle:hover {
    background: var(--bg-primary);
  }
  .error-card {
    background: rgba(248, 81, 73, 0.1);
    border: 1px solid var(--danger);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    color: var(--danger);
    font-size: 13px;
  }
  .round-timing {
    color: var(--text-secondary);
    font-family: var(--font-mono);
    font-size: 11px;
    padding: 2px var(--space-2);
    align-self: flex-start;
  }
  .summary-card {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
  }
  .markdown-body {
    font-size: 14px;
    line-height: 1.7;
    color: var(--text-primary);
  }
  .markdown-body :global(p) { margin: 0 0 var(--space-2) 0; }
  .markdown-body :global(p:last-child) { margin-bottom: 0; }
  .markdown-body :global(ul),
  .markdown-body :global(ol) { margin: var(--space-1) 0; padding-left: var(--space-5); }
  .markdown-body :global(li) { margin: var(--space-1) 0; }
  .markdown-body :global(code) {
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 1px 4px;
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .markdown-body :global(pre) {
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3);
    overflow: auto;
  }
  .markdown-body :global(pre code) {
    background: transparent;
    border: 0;
    padding: 0;
  }
  .markdown-body :global(strong) { font-weight: 600; }
  .markdown-body :global(h1),
  .markdown-body :global(h2),
  .markdown-body :global(h3) { font-size: 15px; margin: var(--space-3) 0 var(--space-2); }
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
