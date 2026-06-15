import { writeFileSync } from "node:fs";
import { join } from "node:path";
import type { AcceptanceReport, AcceptanceStage, BrowserAcceptanceResult } from "./types";

interface BuildReportInput {
  startedAt: string;
  finishedAt: string;
  runDir: string;
  stages: AcceptanceStage[];
  browser: BrowserAcceptanceResult | null;
}

function browserOk(browser: BrowserAcceptanceResult | null): boolean {
  if (!browser) {
    return true;
  }
  return browser.consoleErrors.length === 0 && browser.checks.every((check) => check.ok);
}

function failureSummary(stages: AcceptanceStage[], browser: BrowserAcceptanceResult | null): string {
  const failedStage = stages.find((stage) => !stage.ok);
  if (failedStage) {
    return `${failedStage.name} 失败：${failedStage.summary}`;
  }

  if (browser) {
    const failedCheck = browser.checks.find((check) => !check.ok);
    if (failedCheck) {
      return `browser 失败：${failedCheck.name}${failedCheck.summary ? `：${failedCheck.summary}` : ""}`;
    }
    if (browser.consoleErrors.length > 0) {
      return `browser 失败：控制台出现 ${browser.consoleErrors.length} 条 error`;
    }
  }

  return "";
}

export function buildReport(input: BuildReportInput): AcceptanceReport {
  const ok = input.stages.every((stage) => stage.ok) && browserOk(input.browser);
  return {
    ok,
    startedAt: input.startedAt,
    finishedAt: input.finishedAt,
    runDir: input.runDir,
    stages: input.stages,
    browser: input.browser,
    failureSummary: ok ? "" : failureSummary(input.stages, input.browser),
  };
}

export function writeReport(runDir: string, report: AcceptanceReport): string {
  const path = join(runDir, "report.json");
  writeFileSync(path, `${JSON.stringify(report, null, 2)}\n`, "utf8");
  return path;
}
