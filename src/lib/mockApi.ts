import type { AgentTaskResult } from "./types";

export async function runMockTaskInBrowser(workspaceRoot: string, prompt: string): Promise<AgentTaskResult> {
  void workspaceRoot;

  return {
    summary: "mock Agent 已完成一次文件写入任务。",
    events: [
      { kind: "thought", title: "理解任务", body: prompt },
      { kind: "tool", title: "写入 README.md", body: "已写入 README.md 并生成 diff。" },
      { kind: "summary", title: "任务完成", body: "mock Agent 已生成可展示的文件变更。" },
    ],
    fileChanges: [
      {
        id: "change-1",
        taskRunId: "task-1",
        path: "README.md",
        kind: "modified",
        diff: " # Sophoni\n+Mock task completed for: " + prompt + "\n",
        createdAt: new Date("2026-06-13T00:00:00.000Z").toISOString(),
      },
    ],
  };
}
