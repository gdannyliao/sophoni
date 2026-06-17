<script lang="ts">
  import { onMount } from "svelte";
  import PairingView from "./lib/components/PairingView.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import type { AgentEvent, ConversationSummary } from "./lib/types";
  import { hasConnection, clearConnection, loadConnection } from "./lib/mobile/connection";
  import {
    listConversations,
    getConversation,
    runAgentTask,
  } from "./lib/mobile/mobile-api";

  // 配对状态：未配对显示 PairingView，已配对显示主界面
  let paired = hasConnection();

  // 会话状态
  let conversations: ConversationSummary[] = [];
  let activeConversationId: string | null = null;
  let events: AgentEvent[] = [];
  let prompt = "";
  let running = false;
  let streamingText = "";
  let currentCancel: (() => void) | null = null;

  // token rAF 节流（与桌面端一致，避免高频 token 卡 UI）
  let pendingBuffer = "";
  let rafScheduled = false;

  function scheduleFlush() {
    if (rafScheduled) return;
    rafScheduled = true;
    requestAnimationFrame(() => {
      rafScheduled = false;
      if (pendingBuffer) {
        streamingText += pendingBuffer;
        pendingBuffer = "";
      }
    });
  }

  onMount(async () => {
    if (paired) {
      await refreshConversations();
    }
  });

  async function refreshConversations() {
    try {
      conversations = await listConversations();
    } catch (e) {
      // 401 表示连接失效，回到配对页
      if (String(e).includes("失效") || String(e).includes("401")) {
        handleDisconnect();
      }
    }
  }

  function handlePaired() {
    paired = true;
    refreshConversations();
  }

  function handleDisconnect() {
    clearConnection();
    paired = false;
    conversations = [];
    events = [];
    activeConversationId = null;
  }

  async function selectConversation(id: string) {
    activeConversationId = id;
    try {
      const conv = await getConversation(id);
      const convEvents: AgentEvent[] = JSON.parse(conv.eventsJson || "[]");
      events = convEvents;
        streamingText = "";
    } catch {
      // 加载失败保持空
    }
  }

  async function newConversation() {
    activeConversationId = null;
    events = [];
    streamingText = "";
  }

  async function runTask(task: string) {
    running = true;
    const isNewConversation = activeConversationId === null;
    if (isNewConversation) {
      events = [];
    }
    streamingText = "";
    pendingBuffer = "";
    rafScheduled = false;

    try {
      const { promise, cancel } = runAgentTask(
        task,
        isNewConversation ? null : activeConversationId,
        (e: AgentEvent) => {
          if (e.kind === "token") {
            pendingBuffer += e.body;
            scheduleFlush();
          } else {
            if (e.kind === "thought" || e.kind === "summary") {
              streamingText = "";
              pendingBuffer = "";
              rafScheduled = false;
            }
            if (e.kind === "conversation_created") {
              activeConversationId = e.body;
              if (!conversations.some((c) => c.id === e.body)) {
                conversations = [
                  { id: e.body, title: e.body, updatedAt: new Date().toISOString() },
                  ...conversations,
                ];
              }
            }
            events = [...events, e];
          }
        },
      );
      currentCancel = cancel;

      const result = await promise;
      streamingText = "";
      if (isNewConversation && activeConversationId) {
        conversations = conversations.map((c) =>
          c.id === activeConversationId ? { ...c, title: result.summary || c.title } : c,
        );
      }
    } catch (e) {
      events = [...events, { kind: "error", title: "调用失败", body: String(e), toolCallId: undefined }];
    } finally {
      currentCancel = null;
      running = false;
    }
  }

  async function cancel() {
    currentCancel?.();
  }
</script>

{#if !paired}
  <PairingView onPaired={handlePaired} />
{:else}
  <div class="mobile-shell" data-testid="mobile-app">
    <!-- 顶部：会话切换 + 断开 -->
    <header class="mobile-header">
      <select
        class="conv-select"
        value={activeConversationId ?? ""}
        on:change={(e) => {
          const v = (e.target as HTMLSelectElement).value;
          if (v === "") newConversation();
          else selectConversation(v);
        }}
      >
        <option value="">+ 新对话</option>
        {#each conversations as conv (conv.id)}
          <option value={conv.id}>{conv.title}</option>
        {/each}
      </select>
      <button class="btn-icon" on:click={handleDisconnect} aria-label="断开连接">⏻</button>
    </header>

    <Conversation
      {events}
      {streamingText}
      bind:prompt
      {running}
      workspacePath="移动端"
      changeCount={0}
      onRun={runTask}
      onCancel={cancel}
    />
  </div>
{/if}

<style>
  .mobile-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .mobile-header {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
  }
  .conv-select {
    flex: 1;
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-2) var(--space-3);
    color: var(--text-primary);
    font-size: 14px;
  }
  .btn-icon {
    background: none;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-2);
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 16px;
  }
</style>
