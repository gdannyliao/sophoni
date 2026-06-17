<script lang="ts">
  import { onMount } from "svelte";
  import { listScheduledTasks, updateScheduledTask, deleteScheduledTask } from "../api";
  import type { ScheduledTask } from "../types";

  export let onClose: () => void = () => {};

  let tasks: ScheduledTask[] = [];

  onMount(async () => {
    try {
      tasks = await listScheduledTasks();
    } catch {
      tasks = [];
    }
  });

  async function toggle(id: string, enabled: boolean) {
    await updateScheduledTask(id, !enabled);
    tasks = tasks.map((t) => (t.id === id ? { ...t, enabled: !enabled } : t));
  }

  async function remove(id: string) {
    await deleteScheduledTask(id);
    tasks = tasks.filter((t) => t.id !== id);
  }

  function fmtTime(hour: number, minute: number): string {
    return `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
  }
</script>

<div class="schedule-panel" role="dialog" aria-modal="true" data-testid="schedule-panel">
  <div class="panel-header">
    <h2>⏰ 定时任务</h2>
    <button class="btn icon-only" on:click={onClose}>✕</button>
  </div>
  <div class="panel-body">
    <p class="hint">在对话里说「每天 X 点做 Y」来添加定时任务。</p>
    {#if tasks.length === 0}
      <p class="empty">暂无定时任务</p>
    {:else}
      {#each tasks as task (task.id)}
        <div class="task-row" data-testid="schedule-task">
          <div class="task-info">
            <span class="task-time">每天 {fmtTime(task.hour, task.minute)}</span>
            <span class="task-prompt">{task.prompt}</span>
            {#if task.lastRunAt}
              <span class="task-last">上次: {new Date(task.lastRunAt).toLocaleString()}</span>
            {/if}
          </div>
          <div class="task-actions">
            <button class="btn small-btn" on:click={() => toggle(task.id, task.enabled)}>
              {task.enabled ? "暂停" : "启用"}
            </button>
            <button class="btn small-btn cancel-btn" on:click={() => remove(task.id)}>删除</button>
          </div>
        </div>
      {/each}
    {/if}
  </div>
</div>

<style>
  .schedule-panel {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    min-width: 420px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.4);
  }
  .panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--space-4) var(--space-6);
    border-bottom: 1px solid var(--border);
  }
  .panel-header h2 {
    margin: 0;
    font-size: 16px;
  }
  .panel-body {
    padding: var(--space-4) var(--space-6);
    max-height: 60vh;
    overflow: auto;
  }
  .hint {
    font-size: 12px;
    color: var(--text-secondary);
    margin-bottom: var(--space-3);
  }
  .empty {
    color: var(--text-secondary);
    font-size: 13px;
  }
  .task-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border);
  }
  .task-row:last-child {
    border-bottom: 0;
  }
  .task-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .task-time {
    font-family: var(--font-mono);
    font-size: 14px;
    font-weight: 600;
  }
  .task-prompt {
    font-size: 13px;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .task-last {
    font-size: 11px;
    color: var(--text-secondary);
  }
  .task-actions {
    display: flex;
    gap: var(--space-2);
    flex-shrink: 0;
  }
  .small-btn {
    padding: var(--space-1) var(--space-3);
    font-size: 12px;
  }
  .cancel-btn {
    color: var(--danger);
    border-color: var(--danger);
  }
  .icon-only {
    padding: var(--space-1) var(--space-2);
  }
</style>
