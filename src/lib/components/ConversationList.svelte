<script lang="ts">
  import type { ConversationSummary } from "../types";

  export let conversations: ConversationSummary[] = [];
  export let onSelect: (id: string) => void = () => {};
  export let onNew: () => void = () => {};
  export let onDisconnect: () => void = () => {};

  function formatTime(iso: string): string {
    try {
      const d = new Date(iso);
      const now = new Date();
      const sameDay = d.toDateString() === now.toDateString();
      if (sameDay) {
        return d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
      }
      return d.toLocaleDateString("zh-CN", { month: "numeric", day: "numeric" });
    } catch {
      return "";
    }
  }
</script>

<div class="conv-list-page" data-testid="conv-list-page">
  <!-- 顶栏：标题 + 断开按钮 -->
  <header class="list-header">
    <h1>会话</h1>
    <button class="btn-icon" on:click={onDisconnect} aria-label="断开连接">断开</button>
  </header>

  <!-- 会话列表 -->
  <div class="conv-items">
    {#if conversations.length === 0}
      <div class="empty">还没有会话，点上方开始第一个</div>
    {:else}
      {#each conversations as conv (conv.id)}
        <button
          class="conv-item"
          data-testid="conv-item"
          on:click={() => onSelect(conv.id)}
        >
          <span class="conv-icon">💬</span>
          <span class="conv-body">
            <span class="conv-title">{conv.title}</span>
            {#if conv.updatedAt}
              <span class="conv-time">{formatTime(conv.updatedAt)}</span>
            {/if}
          </span>
        </button>
      {/each}
    {/if}
  </div>

  <!-- 底部新对话栏（固定在底部，避开导航条） -->
  <div class="bottom-bar">
    <button class="btn-new" data-testid="new-conv-btn" on:click={onNew}>
      <span class="plus">＋</span> 新对话
    </button>
  </div>
</div>

<style>
  .conv-list-page {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--bg-primary);
  }
  .list-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: max(env(safe-area-inset-top, 0px), 28px) var(--space-4) var(--space-3);
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
  }
  .list-header h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 700;
  }
  .btn-icon {
    background: none;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-2) var(--space-4);
    color: var(--text-secondary);
    font-size: 14px;
    min-height: 36px;
    white-space: nowrap;
  }
  .btn-new {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    padding: var(--space-3);
    width: 100%;
    background: var(--accent-bg, #4a9eff);
    color: white;
    border: 0;
    border-radius: var(--radius-md);
    font-size: 15px;
    min-height: 44px;
    cursor: pointer;
  }
  .plus { font-size: 18px; line-height: 1; }
  .conv-items {
    flex: 1;
    overflow: auto;
    padding: 0 var(--space-2) max(env(safe-area-inset-bottom, 0px), 16px);
  }
  .empty {
    text-align: center;
    color: var(--text-secondary);
    font-size: 14px;
    padding: var(--space-8) var(--space-4);
  }
  .conv-item {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    padding: var(--space-3) var(--space-4);
    background: none;
    border: 0;
    border-bottom: 1px solid var(--border);
    color: var(--text-primary);
    text-align: left;
    cursor: pointer;
    min-height: 56px;
  }
  .conv-item:active {
    background: var(--bg-secondary);
  }
  .conv-icon { font-size: 18px; flex-shrink: 0; }
  .conv-body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }
  .conv-title {
    font-size: 15px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .conv-time {
    font-size: 12px;
    color: var(--text-secondary);
  }
  .bottom-bar {
    padding: var(--space-3) var(--space-4) max(env(safe-area-inset-bottom, 0px), 12px);
    border-top: 1px solid var(--border);
    background: var(--bg-secondary);
  }
</style>
