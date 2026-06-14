<script lang="ts">
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import ContextPanel from "./lib/components/ContextPanel.svelte";
  import { runMockTaskInBrowser } from "./lib/mockApi";
  import type { AgentEvent, FileChange } from "./lib/types";

  let events: AgentEvent[] = [];
  let fileChanges: FileChange[] = [];
  let summary = "输入任务后，Agent 会在这里展示步骤和结果。";
  let prompt = "";
  let running = false;

  async function runDemo(task: string) {
    running = true;
    try {
      const result = await runMockTaskInBrowser("/tmp/sophoni", task || "生成基础 README");
      events = result.events;
      fileChanges = result.fileChanges;
      summary = result.summary;
    } finally {
      running = false;
    }
  }
</script>

<div class="app-shell">
  <Sidebar />
  <Conversation {events} {summary} bind:prompt {running} onRun={runDemo} />
  <ContextPanel {fileChanges} />
</div>
