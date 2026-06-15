<script lang="ts">
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import ContextPanel from "./lib/components/ContextPanel.svelte";
  import SettingsPanel from "./lib/components/SettingsPanel.svelte";
  import { runAgentTask, cancelAgentTask, onAgentEvent } from "./lib/api";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  import type { AgentEvent, FileChange } from "./lib/types";

  let events: AgentEvent[] = [];
  let fileChanges: FileChange[] = [];
  let summary = "输入任务后，Agent 会在这里展示步骤和结果。";
  let prompt = "";
  let running = false;
  let unlisten: UnlistenFn | null = null;
  let showSettings = false;

  // Hardcoded workspace for MVP — "open workspace" UI is a follow-up plan.
  const WORKSPACE_ROOT = "/tmp/sophoni";

  async function runDemo(task: string) {
    running = true;
    events = [];
    fileChanges = [];
    try {
      unlisten = await onAgentEvent((e) => { events = [...events, e]; });
      const result = await runAgentTask(WORKSPACE_ROOT, task || "读 README.md 并加一行注释");
      // Reconcile with authoritative return value.
      events = result.events;
      fileChanges = result.fileChanges;
      summary = result.summary;
    } catch (e) {
      events = [...events, { kind: "error", title: "调用失败", body: String(e) }];
    } finally {
      unlisten?.();
      unlisten = null;
      running = false;
    }
  }

  async function cancel() {
    await cancelAgentTask();
  }
</script>

<div class="app-shell" data-testid="app-shell">
  <Sidebar />
  <Conversation {events} {summary} bind:prompt {running} onRun={runDemo} onCancel={cancel} />
  <ContextPanel {fileChanges} />
</div>

<button class="settings-toggle" type="button" on:click={() => (showSettings = true)}>设置</button>

{#if showSettings}
  <button
    type="button"
    class="overlay"
    aria-label="关闭设置"
    on:click={() => (showSettings = false)}
  >
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div
      class="overlay-content"
      role="dialog"
      aria-modal="true"
      tabindex="-1"
      on:click|stopPropagation
    >
      <SettingsPanel onClose={() => (showSettings = false)} />
    </div>
  </button>
{/if}
