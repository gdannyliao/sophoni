import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { classifyCommandRisk, getAppStatus, runMockTask } from "./api";
import { runMockTaskInBrowser } from "./mockApi";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
});

describe("api", () => {
  it('calls invoke("get_app_status")', async () => {
    invokeMock.mockResolvedValue("ok");

    await getAppStatus();

    expect(invokeMock).toHaveBeenCalledWith("get_app_status");
  });

  it('calls invoke("classify_command_risk") with command and workspaceRoot', async () => {
    invokeMock.mockResolvedValue("low");

    await classifyCommandRisk("git diff", "/tmp/x");

    expect(invokeMock).toHaveBeenCalledWith("classify_command_risk", {
      command: "git diff",
      workspaceRoot: "/tmp/x",
    });
  });

  it('calls invoke("run_mock_task") with workspaceRoot and prompt', async () => {
    invokeMock.mockResolvedValue({
      summary: "",
      events: [],
      fileChanges: [],
    });

    await runMockTask("/tmp/x", "prompt");

    expect(invokeMock).toHaveBeenCalledWith("run_mock_task", {
      workspaceRoot: "/tmp/x",
      prompt: "prompt",
    });
  });
});

describe("mockApi", () => {
  it("returns a mock task with events and file changes", async () => {
    const result = await runMockTaskInBrowser("/tmp/x", "整理 README");

    expect(result.summary).toContain("mock Agent");
    expect(result.events.some((event) => event.kind === "tool")).toBe(true);
    expect(result.fileChanges[0].path).toBe("README.md");
    expect(result.fileChanges[0].diff).toContain("整理 README");
  });
});
