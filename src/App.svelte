<script lang="ts">
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import ContextPanel from "./lib/components/ContextPanel.svelte";
  import { runMockTaskInBrowser } from "./lib/mockApi";
  import type { AgentEvent, FileChange } from "./lib/types";

  let events: AgentEvent[] = [];
  let fileChanges: FileChange[] = [];
  let summary = "输入任务后，Agent 会在这里展示步骤和结果。";

  async function runDemo() {
    const result = await runMockTaskInBrowser("/tmp/sophoni", "生成基础 README");
    events = result.events;
    fileChanges = result.fileChanges;
    summary = result.summary;
  }
</script>

<div class="app-shell">
  <Sidebar />
  <Conversation {events} {summary} />
  <ContextPanel {fileChanges} />
</div>

<button class="floating-run" type="button" on:click={runDemo}>运行 mock 任务</button>
