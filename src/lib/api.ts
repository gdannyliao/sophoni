import { invoke } from "@tauri-apps/api/core";
import type { AgentTaskResult, CommandRisk } from "./types";

export async function getAppStatus(): Promise<string> {
  return invoke<string>("get_app_status");
}

export async function classifyCommandRisk(command: string, workspaceRoot: string): Promise<CommandRisk> {
  return invoke<CommandRisk>("classify_command_risk", { command, workspaceRoot });
}

export async function runMockTask(workspaceRoot: string, prompt: string): Promise<AgentTaskResult> {
  return invoke<AgentTaskResult>("run_mock_task", { workspaceRoot, prompt });
}
