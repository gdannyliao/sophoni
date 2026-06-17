<script lang="ts">
  import { onMount } from "svelte";
  import PairingView from "./lib/components/PairingView.svelte";
  import ConversationList from "./lib/components/ConversationList.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import type { AgentEvent, ConversationSummary } from "./lib/types";
  import { hasConnection, clearConnection } from "./lib/mobile/connection";
  import {
    listConversations,
    getConversation,
    runAgentTask,
  } from "./lib/mobile/mobile-api";

  // 配对状态
  let paired = hasConnection();

  // 视图导航：列表 ↔ 详情
  type View = "list" | "conversation";
  let view: View = "list";

  // 会话状态
  let conversations: ConversationSummary[] = [];
  let activeConversationId: string | null = null;
  let events: AgentEvent[] = [];
  let prompt = "";
  let running = false;
  let streamingText = "";
  let currentCancel: (() => void) | null = null;

  // token rAF 节流
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
      if (String(e).includes("失效") || String(e).includes("401")) {
        handleDisconnect();
      }
    }
  }

  function handlePaired() {
    paired = true;
    view = "list";
    refreshConversations();
  }

  function handleDisconnect() {
    clearConnection();
    paired = false;
    conversations = [];
    events = [];
    activeConversationId = null;
    view = "list";
  }

  async function openConversation(id: string) {
    activeConversationId = id;
    events = [];
    streamingText = "";
    view = "conversation";
    try {
      const conv = await getConversation(id);
      events = JSON.parse(conv.eventsJson || "[]");
    } catch {
      // 加载失败保留空
    }
  }

  function startNewConversation() {
    activeConversationId = null;
    events = [];
    streamingText = "";
    view = "conversation";
  }

  function backToList() {
    currentCancel?.();
    view = "list";
    refreshConversations();
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

  function cancel() {
    currentCancel?.();
  }
</script>

{#if !paired}
  <PairingView onPaired={handlePaired} />
{:else if view === "list"}
  <ConversationList
    {conversations}
    onSelect={openConversation}
    onNew={startNewConversation}
    onDisconnect={handleDisconnect}
  />
{:else}
  <div class="conv-detail" data-testid="mobile-app">
    <header class="detail-header">
      <button class="back-btn" data-testid="back-btn" on:click={backToList} aria-label="返回列表">←</button>
      <span class="detail-title">
        {activeConversationId ? "会话" : "新对话"}
      </span>
    </header>

    <Conversation
      {events}
      {streamingText}
      bind:prompt
      {running}
      workspacePath="移动端"
      changeCount={0}
      mobile={true}
      onRun={runTask}
      onCancel={cancel}
    />
  </div>
{/if}

<style>
  .conv-detail {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .detail-header {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: max(env(safe-area-inset-top, 0px), 28px) var(--space-3) var(--space-2);
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
  }
  .back-btn {
    background: none;
    border: 0;
    color: var(--accent-bg, #4a9eff);
    font-size: 22px;
    padding: var(--space-2) var(--space-3);
    cursor: pointer;
    min-height: 44px;
    min-width: 44px;
  }
  .detail-title {
    font-size: 16px;
    font-weight: 600;
  }
</style>
