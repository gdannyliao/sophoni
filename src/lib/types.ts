export type CommandRisk = "low" | "high";

export type RiskLevel = "standard" | "relaxed" | "unrestricted";

export interface CommandConfirmRequest {
  requestId: string;
  command: string;
  reason: string;
}

export interface AgentEvent {
  kind: string;
  title: string;
  body: string;
  toolCallId?: string;
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

export interface ConversationSummary {
  id: string;
  title: string;
  updatedAt: string;
}

export interface WorkspaceGroup {
  id: string;
  name: string;
  path: string;
  conversations: ConversationSummary[];
}

export interface Conversation extends ConversationSummary {
  eventsJson: string;
}

export interface SearchConfig {
  tavilyKey: string | null;
  googleKey: string | null;
  googleCx: string | null;
}
