<script lang="ts">
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import ReviewView from "./lib/components/ReviewView.svelte";
  import SettingsPanel from "./lib/components/SettingsPanel.svelte";
  import ConfirmDialog from "./lib/components/ConfirmDialog.svelte";
  import { runAgentTask, cancelAgentTask, onAgentEvent, onCommandConfirm, resolveCommandConfirm } from "./lib/api";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  import type { AgentEvent, CommandConfirmRequest, FileChange } from "./lib/types";

  let events: AgentEvent[] = [];
  let fileChanges: FileChange[] = [];
  let summary = "";
  let prompt = "";
  let running = false;
  let streamingText = "";
  let unlisten: UnlistenFn | null = null;
  let confirmUnlisten: UnlistenFn | null = null;
  let view: "main" | "review" = "main";
  let showSettings = false;
  let sidebarCollapsed = false;
  let pendingConfirm: CommandConfirmRequest | null = null;

  const WORKSPACE_ROOT = "/tmp/sophoni";

  // token 节流：后端已按 30ms 窗口合并，但 IPC 回调仍可能密集到达。前端用 rAF 把
  // 多次 token 累积到一帧内只触发一次 streamingText 赋值（=一次重渲染），彻底避免
  // 高频 token 卡死主线程。pendingBuffer 在帧间累积，rafScheduled 防止重复调度。
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

  async function runDemo(task: string) {
    running = true;
    events = [];
    fileChanges = [];
    summary = "";
    streamingText = "";
    pendingBuffer = "";
    rafScheduled = false;
    try {
      // token 事件（流式增量）走 rAF 节流累积到 streamingText，避免每个 token 都 push
      // 进 events 数组导致 processEvents O(n²) 重算。其他事件仍走 events。
      unlisten = await onAgentEvent((e) => {
        if (e.kind === "token") {
          pendingBuffer += e.body;
          scheduleFlush();
        } else {
          events = [...events, e];
        }
      });
      confirmUnlisten = await onCommandConfirm((req) => { pendingConfirm = req; });
      const result = await runAgentTask(WORKSPACE_ROOT, task || "读 README.md 并加一行注释");
      fileChanges = result.fileChanges;
      summary = result.summary;
      streamingText = ""; // 任务结束，流式文本由 summary/事件定型
    } catch (e) {
      events = [...events, { kind: "error", title: "调用失败", body: String(e), toolCallId: undefined }];
    } finally {
      unlisten?.();
      unlisten = null;
      confirmUnlisten?.();
      confirmUnlisten = null;
      running = false;
    }
  }

  async function cancel() {
    await cancelAgentTask();
  }

  async function resolveConfirm(allowed: boolean) {
    if (pendingConfirm) {
      await resolveCommandConfirm(pendingConfirm.requestId, allowed);
      pendingConfirm = null;
    }
  }
</script>

{#if view === "review"}
  <ReviewView {fileChanges} onClose={() => (view = "main")} />
{:else}
  <div class="app-shell" data-testid="app-shell">
    <Sidebar
      collapsed={sidebarCollapsed}
      onToggleCollapse={() => (sidebarCollapsed = !sidebarCollapsed)}
      onOpenSettings={() => (showSettings = true)}
    />
    <Conversation
      {events}
      {summary}
      {streamingText}
      bind:prompt
      {running}
      workspacePath={WORKSPACE_ROOT}
      changeCount={fileChanges.length}
      onRun={runDemo}
      onCancel={cancel}
      onReview={() => (view = "review")}
    />
  </div>
{/if}

{#if showSettings}
  <button
    type="button"
    class="overlay"
    aria-label="关闭设置"
    on:click={() => (showSettings = false)}
  >
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="overlay-content" role="dialog" aria-modal="true" tabindex="-1" on:click|stopPropagation>
      <SettingsPanel onClose={() => (showSettings = false)} />
    </div>
  </button>
{/if}

{#if pendingConfirm}
  <ConfirmDialog request={pendingConfirm} onResolve={resolveConfirm} />
{/if}

<style>
  .app-shell {
    display: grid;
    grid-template-columns: auto 1fr;
    height: 100vh;
  }
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    border: 0;
    padding: 0;
    width: 100%;
    cursor: default;
    text-align: left;
    font: inherit;
    color: inherit;
  }
  .overlay-content {
    background: transparent;
  }
</style>
