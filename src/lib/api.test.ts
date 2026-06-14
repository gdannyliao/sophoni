import { describe, expect, it } from "vitest";
import { runMockTaskInBrowser } from "./mockApi";

describe("mockApi", () => {
  it("returns a mock task with events and file changes", async () => {
    const result = await runMockTaskInBrowser("整理 README");

    expect(result.summary).toContain("mock Agent");
    expect(result.events.some((event) => event.kind === "tool")).toBe(true);
    expect(result.fileChanges[0].path).toBe("README.md");
  });
});
