import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AgentEvent, AgentTaskResult, CommandConfirmRequest, CommandRisk, ConfigStatus, Conversation, ConversationSummary, RiskLevel, SearchConfig, WorkspaceGroup } from "./types";

export async function getAppStatus(): Promise<string> {
  return invoke<string>("get_app_status");
}

export async function classifyCommandRisk(command: string, workspaceRoot: string): Promise<CommandRisk> {
  return invoke<CommandRisk>("classify_command_risk", { command, workspaceRoot });
}

export async function runMockTask(workspaceRoot: string, prompt: string): Promise<AgentTaskResult> {
  return invoke<AgentTaskResult>("run_mock_task", { workspaceRoot, prompt });
}

export async function runAgentTask(prompt: string, existingConversationId: string | null): Promise<AgentTaskResult> {
  return invoke<AgentTaskResult>("run_agent_task", { prompt, existingConversationId });
}

export async function cancelAgentTask(): Promise<void> {
  await invoke("cancel_agent_task");
}

export async function getConfigStatus(): Promise<ConfigStatus> {
  return invoke<ConfigStatus>("get_config_status");
}

export async function onAgentEvent(cb: (e: AgentEvent) => void): Promise<UnlistenFn> {
  return listen<AgentEvent>("agent-event", (ev) => cb(ev.payload));
}

export async function getRiskLevel(): Promise<RiskLevel> {
  return invoke<RiskLevel>("get_risk_level");
}

export async function setRiskLevel(level: RiskLevel): Promise<void> {
  await invoke("set_risk_level", { level });
}

export async function resolveCommandConfirm(requestId: string, allowed: boolean): Promise<void> {
  await invoke("resolve_command_confirm", { requestId, allowed });
}

export async function onCommandConfirm(cb: (req: CommandConfirmRequest) => void): Promise<UnlistenFn> {
  return listen<CommandConfirmRequest>("command-confirm", (ev) => cb(ev.payload));
}

export async function getWorkspacePath(): Promise<string | null> {
  return invoke<string | null>("get_workspace_path");
}

export async function setWorkspacePath(path: string): Promise<void> {
  await invoke("set_workspace_path", { path });
}

export async function listConversations(): Promise<ConversationSummary[]> {
  return invoke<ConversationSummary[]>("list_conversations");
}

/** 列出所有工作区及其会话（按工作区分组，Sidebar 多工作区视图用）。 */
export async function listConversationsGrouped(): Promise<WorkspaceGroup[]> {
  return invoke<WorkspaceGroup[]>("list_conversations_grouped");
}

export async function getConversation(id: string): Promise<Conversation> {
  return invoke<Conversation>("get_conversation", { id });
}

export async function deleteConversation(id: string): Promise<void> {
  await invoke("delete_conversation", { id });
}

export async function getSearchConfig(): Promise<SearchConfig> {
  return invoke<SearchConfig>("get_search_config");
}

export async function saveSearchConfig(config: SearchConfig): Promise<void> {
  await invoke("save_search_config", {
    tavilyKey: config.tavilyKey,
    googleKey: config.googleKey,
    googleCx: config.googleCx,
  });
}

/** 获取配对二维码（供桌面端「手机连接」面板展示）。含 IP/端口/配对码 + 二维码 SVG。 */
export async function getPairQrcode(): Promise<PairQrCode> {
  return invoke<PairQrCode>("get_pair_qrcode");
}

export interface PairQrCode {
  url: string;
  svg: string;
  ip: string;
  port: number;
  code: string;
}
