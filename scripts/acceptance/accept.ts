import { relative } from "node:path";
import { runBrowserAcceptance } from "./browser-check";
import { createAcceptanceLogger } from "./logger";
import { buildReport, writeReport } from "./report";
import { runCommand } from "./run-command";
import type { AcceptanceStage } from "./types";

const commands = [
  { name: "pnpm check", command: "pnpm", args: ["check"] },
  { name: "pnpm test", command: "pnpm", args: ["test"] },
  {
    name: "cargo test --manifest-path src-tauri/Cargo.toml",
    command: "cargo",
    args: ["test", "--manifest-path", "src-tauri/Cargo.toml"],
  },
];

async function main(): Promise<void> {
  const startedAt = new Date().toISOString();
  const logger = createAcceptanceLogger();
  const stages: AcceptanceStage[] = [];

  for (const item of commands) {
    stages.push(await runCommand(item.name, item.command, item.args, logger));
  }

  const browser = await runBrowserAcceptance({ logger });

  const report = buildReport({
    startedAt,
    finishedAt: new Date().toISOString(),
    runDir: logger.runDir,
    stages,
    browser,
  });
  const reportPath = writeReport(logger.runDir, report);
  const relativeReportPath = relative(process.cwd(), reportPath) || reportPath;

  console.log(`Acceptance report: ${relativeReportPath}`);
  if (!report.ok) {
    console.error(report.failureSummary);
  }
  process.exitCode = report.ok ? 0 : 1;
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(message);
  process.exitCode = 1;
});
