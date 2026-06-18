<script lang="ts">
  import { onMount } from "svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import ReviewView from "./lib/components/ReviewView.svelte";
  import SettingsPanel from "./lib/components/SettingsPanel.svelte";
  import SchedulePanel from "./lib/components/SchedulePanel.svelte";
  import MobilePairPanel from "./lib/components/MobilePairPanel.svelte";
  import ConfirmDialog from "./lib/components/ConfirmDialog.svelte";
  import WelcomeView from "./lib/components/WelcomeView.svelte";
  import { open, confirm } from "@tauri-apps/plugin-dialog";
  import { runAgentTask, cancelAgentTask, onAgentEvent, onCommandConfirm, resolveCommandConfirm, getWorkspacePath, setWorkspacePath, listConversationsGrouped, getConversation, deleteConversation, getConfigStatus } from "./lib/api";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  import type { AgentEvent, CommandConfirmRequest, ConfigStatus, ConversationSummary, FileChange, WorkspaceGroup } from "./lib/types";

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
  let showSchedule = false;
  let showMobilePair = false;
  let sidebarCollapsed = false;
  // 配置状态缓存：onMount 查询一次，发送任务前据此拦截；用户改完配置关闭面板后刷新。
  let configStatus: ConfigStatus | null = null;
  let configError: string | null = null;
  let pendingConfirm: CommandConfirmRequest | null = null;
  let workspacePath: string | null = null;
  let groups: WorkspaceGroup[] = [];
  let conversations: ConversationSummary[] = [];
  let activeConversationId: string | null = null;

  onMount(async () => {
    try {
      workspacePath = await getWorkspacePath();
    } catch {
      workspacePath = null;
    }
    await refreshConfig();
    await refreshGroups();
  });

  /** 查询 LLM 配置状态；未配置时设置 configError 提示文案。 */
  async function refreshConfig() {
    try {
      configStatus = await getConfigStatus();
    } catch {
      configStatus = { configured: false, provider: "(未配置)", model: "(未配置)" };
    }
    configError = configStatus?.configured ? null : "未配置 LLM，无法发送任务。请在 ~/.config/sophoni/config.toml 配置 Provider。";
  }

  /** 关闭设置面板并刷新配置状态（用户可能刚改完配置）。 */
  async function closeSettings() {
    showSettings = false;
    await refreshConfig();
  }

  /** 加载所有工作区分组会话，并拍平出 conversations 给 WelcomeView 用。 */
  async function refreshGroups() {
    try {
      const raw = await listConversationsGrouped();
      groups = mergeSubdirWorkspaces(raw);
      conversations = groups.flatMap((g) => g.conversations);
    } catch {
      groups = [];
      conversations = [];
    }
  }

  /** 乐观插入/更新一个会话到 groups（按 workspacePath 归组）和 conversations 列表。
   *  用于发送新会话消息时立即在侧边栏显示占位项，不等待后端 conversation_created。 */
  function upsertConversation(conv: ConversationSummary) {
    if (conversations.some((c) => c.id === conv.id)) {
      conversations = conversations.map((c) => (c.id === conv.id ? { ...c, ...conv } : c));
    } else {
      conversations = [conv, ...conversations];
    }
    // 归入 workspacePath 匹配的分组；找不到则创建临时分组
    const norm = (p: string) => p.replace(/\/+$/, "");
    const targetPath = norm(conv.workspacePath);
    let group = groups.find((g) => norm(g.path) === targetPath);
    if (!group) {
      group = { id: targetPath || "chat", name: "", path: conv.workspacePath, conversations: [] };
      groups = [...groups, group];
    }
    if (group.conversations.some((c) => c.id === conv.id)) {
      group.conversations = group.conversations.map((c) => (c.id === conv.id ? { ...c, ...conv } : c));
    } else {
      group.conversations = [conv, ...group.conversations];
    }
    groups = [...groups];
  }

  /** 把占位会话的临时 id 替换为后端返回的真实 id（groups + conversations + activeConversationId）。 */
  function replaceConversationId(oldId: string, newId: string) {
    conversations = conversations.map((c) => (c.id === oldId ? { ...c, id: newId } : c));
    for (const g of groups) {
      g.conversations = g.conversations.map((c) => (c.id === oldId ? { ...c, id: newId } : c));
    }
    groups = [...groups];
    if (activeConversationId === oldId) {
      activeConversationId = newId;
    }
  }

  /**
   * 把子目录工作区归并到父目录工作区（UI 层归并，不改数据）。
   * 例：/a/b/deploy 的会话合并进 /a/b（若 /a/b 也存在工作区）。
   * 规则：路径按长度从长到短排序，每个工作区找是否存在更短的父路径工作区；
   * 存在则把会话并入父，自身不单独成组。归并后按 updatedAt 倒序重排各组会话。
   */
  function mergeSubdirWorkspaces(raw: WorkspaceGroup[]): WorkspaceGroup[] {
    // 规范化：去尾斜杠，保证父子判定用统一前缀（/a/b 而非 /a/b/）
    const norm = (p: string) => p.replace(/\/+$/, "");
    const isSubdir = (child: string, parent: string) => {
      const c = norm(child);
      const p = norm(parent);
      return c !== p && c.startsWith(p + "/");
    };

    // 按路径长度从短到长排序：父目录先入集合，子目录后处理时才能找到父归并。
    const sorted = [...raw].sort(
      (a, b) => norm(a.path).length - norm(b.path).length,
    );

    const merged = new Map<string, WorkspaceGroup>();
    for (const g of sorted) {
      // 找已入集合的最长父路径工作区，归并过去（多级嵌套时归到最近父）
      const parentCandidates = [...merged.keys()].filter((k) => isSubdir(g.path, k));
      const parentKey = parentCandidates.sort((a, b) => b.length - a.length)[0];
      if (parentKey !== undefined) {
        const parent = merged.get(parentKey)!;
        parent.conversations = [...parent.conversations, ...g.conversations].sort(
          (a, b) => b.updatedAt.localeCompare(a.updatedAt),
        );
      } else {
        merged.set(norm(g.path), { ...g, path: norm(g.path) });
      }
    }

    return [...merged.values()];
  }

  async function selectWorkspace() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") {
      await setWorkspacePath(selected);
      workspacePath = selected;
      await refreshGroups();
    }
  }

  async function clearWorkspace() {
    await setWorkspacePath("");
    workspacePath = null;
  }

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
    // 前置校验：未配置 LLM 时直接拦截，避免请求打到后端再静默失败。
    if (!configStatus?.configured) {
      configError = "未配置 LLM，无法发送任务。请在 ~/.config/sophoni/config.toml 配置 Provider。";
      return;
    }
    running = true;
    // 续聊（已有 activeConversationId）时保留历史 events，让连续消息累加显示在同一会话流里；
    // 新会话才清空。streamingText/fileChanges 每轮重置。
    const isNewConversation = activeConversationId === null;
    // 任务启动时用户所在会话。新会话任务收到 conversation_created 后更新为真正的新会话 id。
    // 用户若在此期间手动切换会话，activeConversationId 会与之不同——后续事件副作用应跳过，
    // 避免新任务的执行过程覆盖用户当前查看的会话界面。
    let taskConversationId = activeConversationId;
    if (isNewConversation) {
      events = [];
      // 乐观插入占位会话：发送新会话消息时立即在侧边栏显示，不等后端 conversation_created
      // （该事件到达前若用户切走，会以为会话丢失）。收到真实 id 后用 upsertConversation 替换。
      const placeholderId = `pending-${Date.now()}`;
      upsertConversation({ id: placeholderId, title: task.slice(0, 20) || "新对话", updatedAt: new Date().toISOString(), workspacePath: workspacePath ?? "" });
      taskConversationId = placeholderId;
      activeConversationId = placeholderId;
    }
    fileChanges = [];
    summary = "";
    streamingText = "";
    pendingBuffer = "";
    rafScheduled = false;
    try {
      // token 事件（流式增量）走 rAF 节流累积到 streamingText，避免每个 token 都 push
      // 进 events 数组导致 processEvents O(n²) 重算。其他事件仍走 events。
      unlisten = await onAgentEvent((e) => {
        if (e.kind === "conversation_created") {
          const realId = e.body;
          if (isNewConversation) {
            // 用户是否仍停留在占位会话：替换前 taskConversationId 是占位 id。
            const stillOnPlaceholder = activeConversationId === taskConversationId;
            // 用真实 id 替换占位会话（新会话场景 taskConversationId 已是占位 id，非空）
            replaceConversationId(taskConversationId!, realId);
            taskConversationId = realId;
            // 用户没切走时同步展示会话；切走了则保留用户的选择
            if (stillOnPlaceholder) {
              activeConversationId = realId;
            }
          } else if (!conversations.some((c) => c.id === realId)) {
            // 续聊任务后端也会发此事件，已有则跳过
            upsertConversation({ id: realId, title: realId, updatedAt: new Date().toISOString(), workspacePath: workspacePath ?? "" });
          }
        }
        // 用户若已手动切换到别的会话，后续事件不再追加到当前展示界面
        // （事件仍由后端正常处理并落库，切回该会话时能看到）。
        if (activeConversationId !== taskConversationId) {
          return;
        }
        if (e.kind === "token") {
          pendingBuffer += e.body;
          scheduleFlush();
        } else {
          if (e.kind === "thought" || e.kind === "summary") {
            streamingText = "";
            pendingBuffer = "";
            rafScheduled = false;
          }
          events = [...events, e];
        }
      });
      confirmUnlisten = await onCommandConfirm((req) => { pendingConfirm = req; });
      const result = await runAgentTask(task, isNewConversation ? null : activeConversationId);
      fileChanges = result.fileChanges;
      summary = result.summary;
      streamingText = ""; // 任务结束，流式文本由 summary/事件定型
      // 任务结束后刷新分组数据，同步 Sidebar（新会话归入工作区、标题更新）
      await refreshGroups();
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

  function newConversation() {
    activeConversationId = null;
    events = [];
    fileChanges = [];
    summary = "";
    streamingText = "";
  }

  async function handleDeleteConversation(id: string) {
    const confirmed = await confirm("确定删除此会话？", { title: "删除会话", kind: "warning" });
    if (!confirmed) return;
    try {
      await deleteConversation(id);
      await refreshGroups();
      if (activeConversationId === id) {
        newConversation();
      }
    } catch (e) {
      events = [...events, { kind: "error", title: "删除失败", body: String(e), toolCallId: undefined }];
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
      onOpenSchedule={() => (showSchedule = true)}
      onOpenMobilePair={() => (showMobilePair = true)}
      {groups}
      {activeConversationId}
      onSelectConversation={selectConversation}
      onNewConversation={newConversation}
      onDeleteConversation={handleDeleteConversation}
    />
    {#if activeConversationId === null}
      <WelcomeView
        {workspacePath}
        {conversations}
        {configError}
        onStart={runDemo}
        onSelectConversation={selectConversation}
        onSelectWorkspace={selectWorkspace}
        onClearWorkspace={clearWorkspace}
        onOpenSettings={() => (showSettings = true)}
      />
    {:else}
      <Conversation
        {events}
        {streamingText}
        bind:prompt
        {running}
        workspacePath={conversations.find((c) => c.id === activeConversationId)?.workspacePath ?? workspacePath ?? "未选择工作区"}
        changeCount={fileChanges.length}
        title={conversations.find((c) => c.id === activeConversationId)?.title ?? ""}
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
    on:click={closeSettings}
  >
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="overlay-content" role="dialog" aria-modal="true" tabindex="-1" on:click|stopPropagation>
      <SettingsPanel onClose={closeSettings} />
    </div>
  </button>
{/if}

{#if showSchedule}
  <button
    type="button"
    class="overlay"
    aria-label="关闭定时任务"
    on:click={() => (showSchedule = false)}
  >
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="overlay-content" role="dialog" aria-modal="true" tabindex="-1" on:click|stopPropagation>
      <SchedulePanel onClose={() => (showSchedule = false)} />
    </div>
  </button>
{/if}

{#if showMobilePair}
  <MobilePairPanel onClose={() => (showMobilePair = false)} />
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
