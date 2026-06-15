export type CommandRisk = "low" | "high";

export interface AgentEvent {
  kind: string;
  title: string;
  body: string;
}

export interface FileChange {
  id: string;
  taskRunId: string;
  path: string;
  kind: "created" | "modified" | "deleted";
  diff: string;
  createdAt: string;
}

export interface AgentTaskResult {
  summary: string;
  events: AgentEvent[];
  fileChanges: FileChange[];
}

export interface ConfigStatus {
  configured: boolean;
  provider: string;
  model: string;
}
