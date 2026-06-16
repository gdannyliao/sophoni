<script lang="ts">
  import type { ConversationSummary } from "../types";

  export let workspacePath: string | null = null;
  export let conversations: ConversationSummary[] = [];
  export let onStart: (prompt: string) => void = () => {};
  export let onSelectConversation: (id: string) => void = () => {};
  export let onSelectWorkspace: () => void = () => {};

  let prompt = "";

  function handleSubmit() {
    if (prompt.trim()) {
      onStart(prompt);
    }
  }
</script>

<div class="welcome" data-testid="welcome-view">
  <div class="welcome-content">
    <div class="logo">◈</div>
    <h1>开始新对话</h1>
    <p class="subtitle">
      {#if workspacePath}
        Agent 可以读写文件、执行命令、验证代码。
      {:else}
        输入任务开始对话。需要读写文件时，请在左侧选择工作区。
      {/if}
    </p>

    <div class="input-card" data-testid="welcome-input-card">
      <textarea
        data-testid="welcome-input"
        aria-label="任务描述"
        placeholder="描述你想让 Agent 做什么..."
        bind:value={prompt}
        on:keydown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            handleSubmit();
          }
        }}
      ></textarea>
      <div class="input-footer">
        <span class="mode-label" class:full={!!workspacePath}>
          {#if workspacePath}✓ 全功能模式{:else}💬 纯对话模式{/if}
        </span>
        <button class="btn btn-primary" data-testid="welcome-start" on:click={handleSubmit}>开始</button>
      </div>
    </div>

    <div class="workspace-card" data-testid="welcome-workspace">
      <span class="ws-icon">📁</span>
      <div class="ws-info">
        {#if workspacePath}
          <div class="ws-label">当前工作区</div>
          <div class="ws-path" title={workspacePath}>{workspacePath}</div>
        {:else}
          <div class="ws-label">未选择工作区</div>
          <div class="ws-hint">选择工作区以启用文件读写和命令执行</div>
        {/if}
      </div>
      {#if !workspacePath}
        <button class="btn ws-select-btn" data-testid="welcome-select-workspace" on:click={onSelectWorkspace}>选择</button>
      {/if}
    </div>

    {#if workspacePath && conversations.length > 0}
      <div class="recent-section">
        <div class="recent-label">最近会话</div>
        {#each conversations.slice(0, 5) as conv (conv.id)}
          <div
            class="recent-item"
            role="button"
            tabindex="0"
            on:click={() => onSelectConversation(conv.id)}
            on:keydown={(e) => e.key === "Enter" && onSelectConversation(conv.id)}
          >
            <span>💬</span> {conv.title}
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .welcome {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100vh;
    overflow: auto;
    padding: var(--space-6);
  }
  .welcome-content {
    display: flex;
    flex-direction: column;
    align-items: center;
    max-width: 560px;
    width: 100%;
  }
  .logo {
    font-size: 42px;
    color: var(--accent);
    margin-bottom: var(--space-2);
  }
  h1 {
    font-size: 22px;
    font-weight: 700;
    margin: 0 0 var(--space-2) 0;
  }
  .subtitle {
    font-size: 13px;
    color: var(--text-secondary);
    text-align: center;
    margin: 0 0 var(--space-6) 0;
    max-width: 400px;
  }
  .input-card {
    width: 100%;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: var(--space-4);
    margin-bottom: var(--space-3);
  }
  textarea {
    width: 100%;
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    color: var(--text-primary);
    font-family: var(--font-sans);
    font-size: 14px;
    min-height: 56px;
    resize: vertical;
  }
  textarea::placeholder { color: var(--text-secondary); }
  textarea:focus { outline: none; border-color: var(--accent); }
  .input-footer {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-3);
  }
  .mode-label {
    font-size: 11px;
    color: var(--text-secondary);
  }
  .mode-label.full { color: var(--success); }
  .workspace-card {
    width: 100%;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: var(--space-3) var(--space-4);
  }
  .ws-icon { font-size: 18px; }
  .ws-info { flex: 1; overflow: hidden; }
  .ws-label { font-size: 13px; color: var(--text-secondary); }
  .ws-path {
    font-size: 12px;
    font-family: var(--font-mono);
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ws-hint { font-size: 11px; color: var(--accent); }
  .ws-select-btn { font-size: 12px; }
  .recent-section {
    width: 100%;
    margin-top: var(--space-4);
  }
  .recent-label {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--text-secondary);
    letter-spacing: 0.5px;
    margin-bottom: var(--space-2);
  }
  .recent-item {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-2) var(--space-4);
    font-size: 13px;
    cursor: pointer;
    margin-bottom: var(--space-1);
  }
  .recent-item:hover { background: var(--bg-tertiary); }
</style>
