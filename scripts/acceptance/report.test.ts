import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { buildReport, writeReport } from "./report";
import type { AcceptanceStage } from "./types";

let tempDirs: string[] = [];

function tempRunDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "sophoni-accept-report-"));
  tempDirs.push(dir);
  return dir;
}

afterEach(() => {
  for (const dir of tempDirs) {
    rmSync(dir, { recursive: true, force: true });
  }
  tempDirs = [];
});

describe("acceptance report", () => {
  it("marks report ok only when every stage and browser check passes", () => {
    const stages: AcceptanceStage[] = [
      { name: "pnpm check", ok: true, durationMs: 12, summary: "0 errors", logPath: "stdout.log" },
      { name: "pnpm test", ok: true, durationMs: 20, summary: "5 passed", logPath: "stdout.log" },
    ];

    const report = buildReport({
      startedAt: "2026-06-15T00:00:00.000Z",
      finishedAt: "2026-06-15T00:00:01.000Z",
      runDir: ".sophoni/runs/20260615-000000",
      stages,
      browser: {
        url: "http://127.0.0.1:5173",
        screenshotPath: "browser.png",
        consoleErrors: [],
        checks: [{ name: "app shell exists", ok: true }],
      },
    });

    expect(report.ok).toBe(true);
    expect(report.failureSummary).toBe("");
  });

  it("uses the first failed stage as failure summary", () => {
    const report = buildReport({
      startedAt: "2026-06-15T00:00:00.000Z",
      finishedAt: "2026-06-15T00:00:01.000Z",
      runDir: ".sophoni/runs/20260615-000000",
      stages: [
        { name: "pnpm check", ok: true, durationMs: 12, summary: "0 errors", logPath: "stdout.log" },
        { name: "pnpm test", ok: false, durationMs: 20, summary: "1 failed", logPath: "stderr.log" },
      ],
      browser: null,
    });

    expect(report.ok).toBe(false);
    expect(report.failureSummary).toBe("pnpm test 失败：1 failed");
  });

  it("writes report.json with stable camelCase fields", () => {
    const runDir = tempRunDir();
    const report = buildReport({
      startedAt: "2026-06-15T00:00:00.000Z",
      finishedAt: "2026-06-15T00:00:01.000Z",
      runDir,
      stages: [],
      browser: null,
    });

    const path = writeReport(runDir, report);
    const json = JSON.parse(readFileSync(path, "utf8"));

    expect(path.endsWith("report.json")).toBe(true);
    expect(json).toHaveProperty("startedAt");
    expect(json).toHaveProperty("finishedAt");
    expect(json).toHaveProperty("failureSummary");
  });
});
