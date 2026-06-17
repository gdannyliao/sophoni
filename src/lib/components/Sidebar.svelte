<script lang="ts">
  import { getConfigStatus } from "../api";
  import type { ConfigStatus, WorkspaceGroup } from "../types";
  import { onMount } from "svelte";

  export let collapsed = false;
  export let onToggleCollapse: () => void = () => {};
  export let onOpenSettings: () => void = () => {};
  export let onOpenMobilePair: () => void = () => {};
  export let groups: WorkspaceGroup[] = [];
  export let activeConversationId: string | null = null;
  export let onSelectConversation: (id: string) => void = () => {};
  export let onNewConversation: () => void = () => {};
  export let onDeleteConversation: (id: string) => void = () => {};

  let status: ConfigStatus | null = null;

  onMount(async () => {
    try {
      status = await getConfigStatus();
    } catch {
      status = { configured: false, provider: "(未配置)", model: "(未知)" };
    }
  });

  /** 工作区显示名：优先 name，否则用 path 的最后一段目录名。 */
  function wsName(g: WorkspaceGroup): string {
    if (g.name) return g.name;
    const parts = g.path.split("/").filter(Boolean);
    return parts[parts.length - 1] || g.path;
  }
</script>

<aside class="sidebar" class:collapsed aria-label="工作区与会话" data-testid="sidebar">
  {#if !collapsed}
    <div class="sidebar-content">
      <div class="brand-row">
        <div class="brand">◈ Sophoni</div>
        <button class="btn new-btn" data-testid="new-conversation" on:click={onNewConversation} title="新建会话">+</button>
      </div>
      {#if groups.length === 0}
        <div class="session-empty">暂无会话</div>
      {:else}
        {#each groups as group (group.id)}
          <div class="ws-group">
            <div class="ws-group-header" title={group.path}>{wsName(group)}</div>
            {#each group.conversations as conv (conv.id)}
              <div
                class="session-item"
                class:active={conv.id === activeConversationId}
              >
                <span class="session-title" role="button" tabindex="0" on:click={() => onSelectConversation(conv.id)} on:keydown={(e) => e.key === "Enter" && onSelectConversation(conv.id)}>
                  {conv.title.length > 12 ? conv.title.slice(0, 12) + "…" : conv.title}
                </span>
                <button class="delete-btn" data-testid="delete-conversation" on:click={() => onDeleteConversation(conv.id)} title="删除">✕</button>
              </div>
            {/each}
          </div>
        {/each}
      {/if}
    </div>
    <div class="sidebar-footer">
      <div class="model-info">{status?.model ?? "..."}</div>
      <div class="footer-actions">
        <button class="btn icon-footer-btn" data-testid="settings-button" on:click={onOpenSettings} title="设置">⚙</button>
        <button class="btn icon-footer-btn" data-testid="mobile-pair-button" on:click={onOpenMobilePair} title="手机连接">📱</button>
      </div>
    </div>
  {:else}
    <div class="sidebar-collapsed-content">
      <button class="icon-btn" on:click={onToggleCollapse} title="展开">◈</button>
    </div>
  {/if}
</aside>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    background: var(--bg-secondary);
    border-right: 1px solid var(--border);
    transition: width 0.15s;
    overflow: hidden;
  }
  .sidebar:not(.collapsed) { width: 220px; }
  .sidebar.collapsed { width: 48px; }
  .sidebar-content { flex: 1; padding: var(--space-4) var(--space-3); overflow-y: auto; }
  .sidebar-collapsed-content { flex: 1; display: flex; flex-direction: column; align-items: center; padding-top: var(--space-4); }
  .brand-row { display: flex; align-items: center; justify-content: space-between; margin-bottom: var(--space-4); }
  .brand { font-size: 15px; font-weight: 700; color: var(--accent); }
  .new-btn { padding: 2px 8px; font-size: 16px; line-height: 1; }
  .ws-group { margin-bottom: var(--space-4); }
  .ws-group-header {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--text-secondary);
    margin-bottom: var(--space-2);
    letter-spacing: 0.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .session-item {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    color: var(--text-secondary);
    margin-bottom: 2px;
  }
  .session-item:hover { background: var(--bg-tertiary); }
  .session-item.active {
    background: rgba(31, 111, 235, 0.15);
    border-left: 2px solid var(--accent);
    color: var(--text-primary);
  }
  .session-title {
    flex: 1;
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .delete-btn {
    border: 0;
    background: transparent;
    color: var(--text-secondary);
    font-size: 11px;
    cursor: pointer;
    opacity: 0;
    padding: 2px 4px;
    border-radius: var(--radius-sm);
  }
  .session-item:hover .delete-btn { opacity: 1; }
  .delete-btn:hover { color: var(--danger); }
  .session-empty {
    padding: var(--space-2) var(--space-3);
    color: var(--text-secondary);
    font-size: 12px;
  }
  .model-info { font-size: 11px; color: var(--text-secondary); margin-bottom: var(--space-2); }
  .footer-actions { display: flex; gap: var(--space-2); }
  .icon-footer-btn {
    flex: 1;
    padding: var(--space-2);
    font-size: 16px;
    text-align: center;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-primary);
    color: var(--text-secondary);
    cursor: pointer;
  }
  .icon-footer-btn:hover { background: var(--bg-tertiary); color: var(--text-primary); }
  .icon-btn {
    border: 0;
    background: transparent;
    color: var(--accent);
    font-size: 18px;
    cursor: pointer;
  }
</style>
