<script lang="ts">
  import { onMount } from "svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import ReviewView from "./lib/components/ReviewView.svelte";
  import SettingsPanel from "./lib/components/SettingsPanel.svelte";
  import ConfirmDialog from "./lib/components/ConfirmDialog.svelte";
  import WelcomeView from "./lib/components/WelcomeView.svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { runAgentTask, cancelAgentTask, onAgentEvent, onCommandConfirm, resolveCommandConfirm, getWorkspacePath, setWorkspacePath, listConversations, getConversation } from "./lib/api";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  import type { AgentEvent, CommandConfirmRequest, ConversationSummary, FileChange } from "./lib/types";

  let events: AgentEvent[] = [];
  let fileChanges: FileChange[] = [];
  let summary = "";
  let prompt = "";
  let running = false;
  let unlisten: UnlistenFn | null = null;
  let confirmUnlisten: UnlistenFn | null = null;
  let view: "main" | "review" = "main";
  let showSettings = false;
  let sidebarCollapsed = false;
  let pendingConfirm: CommandConfirmRequest | null = null;
  let workspacePath: string | null = null;
  let conversations: ConversationSummary[] = [];
  let activeConversationId: string | null = null;

  onMount(async () => {
    try {
      workspacePath = await getWorkspacePath();
      if (workspacePath) {
        conversations = await listConversations();
      }
    } catch {
      workspacePath = null;
    }
  });

  async function selectWorkspace() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") {
      await setWorkspacePath(selected);
      workspacePath = selected;
      conversations = await listConversations();
    }
  }

  async function runDemo(task: string) {
    running = true;
    events = [];
    fileChanges = [];
    summary = "";
    try {
      unlisten = await onAgentEvent((e) => {
        if (e.kind === "conversation_created") {
          activeConversationId = e.body;
          conversations = [{ id: e.body, title: e.body, updatedAt: new Date().toISOString() }, ...conversations];
        }
        events = [...events, e];
      });
      confirmUnlisten = await onCommandConfirm((req) => { pendingConfirm = req; });
      const result = await runAgentTask(task);
      fileChanges = result.fileChanges;
      summary = result.summary;
      if (activeConversationId) {
        conversations = conversations.map((c) =>
          c.id === activeConversationId
            ? { ...c, title: result.summary || c.title }
            : c
        );
      }
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

  async function selectConversation(id: string) {
    try {
      const conv = await getConversation(id);
      activeConversationId = id;
      events = JSON.parse(conv.eventsJson);
      summary = events.find((e) => e.kind === "summary")?.body ?? "";
      fileChanges = [];
    } catch (e) {
      events = [...events, { kind: "error", title: "加载失败", body: String(e), toolCallId: undefined }];
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
      {workspacePath}
      onSelectWorkspace={selectWorkspace}
      {conversations}
      {activeConversationId}
      onSelectConversation={selectConversation}
    />
    {#if activeConversationId === null}
      <WelcomeView
        {workspacePath}
        {conversations}
        onStart={runDemo}
        onSelectConversation={selectConversation}
        onSelectWorkspace={selectWorkspace}
      />
    {:else}
      <Conversation
        {events}
        {summary}
        bind:prompt
        {running}
        workspacePath={workspacePath ?? "未选择工作区"}
        changeCount={fileChanges.length}
        onRun={runDemo}
        onCancel={cancel}
        onReview={() => (view = "review")}
      />
    {/if}
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
