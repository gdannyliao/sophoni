import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AgentEvent, AgentTaskResult, CommandRisk, ConfigStatus } from "./types";

export async function getAppStatus(): Promise<string> {
  return invoke<string>("get_app_status");
}

export async function classifyCommandRisk(command: string, workspaceRoot: string): Promise<CommandRisk> {
  return invoke<CommandRisk>("classify_command_risk", { command, workspaceRoot });
}

export async function runMockTask(workspaceRoot: string, prompt: string): Promise<AgentTaskResult> {
  return invoke<AgentTaskResult>("run_mock_task", { workspaceRoot, prompt });
}

export async function runAgentTask(workspaceRoot: string, prompt: string): Promise<AgentTaskResult> {
  return invoke<AgentTaskResult>("run_agent_task", { workspaceRoot, prompt });
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
