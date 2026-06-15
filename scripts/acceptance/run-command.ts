import { spawn } from "node:child_process";
import { performance } from "node:perf_hooks";
import type { AcceptanceLogger } from "./logger";
import type { AcceptanceStage } from "./types";

interface RunCommandOptions {
  cwd?: string;
  env?: NodeJS.ProcessEnv;
}

const MAX_SUMMARY_LENGTH = 240;

function lastNonEmptyLine(text: string): string {
  const lines = text.split(/\r?\n/);
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const line = lines[index]?.trim();
    if (line) {
      return line;
    }
  }
  return "";
}

function truncateSummary(summary: string): string {
  if (summary.length <= MAX_SUMMARY_LENGTH) {
    return summary;
  }
  return summary.slice(0, MAX_SUMMARY_LENGTH);
}

function commandLine(command: string, args: string[]): string {
  return [command, ...args].join(" ");
}

function stageSummary(ok: boolean, stdout: string, stderr: string): string {
  const output = ok ? stdout : stderr || stdout;
  const summary = lastNonEmptyLine(output);
  if (summary) {
    return truncateSummary(summary);
  }
  return ok ? "命令执行成功" : "命令执行失败";
}

export function runCommand(
  name: string,
  command: string,
  args: string[],
  logger: AcceptanceLogger,
  options: RunCommandOptions = {},
): Promise<AcceptanceStage> {
  const startedAt = performance.now();
  let stdout = "";
  let stderr = "";

  logger.info(name, `开始执行：${commandLine(command, args)}`);

  return new Promise((resolve) => {
    let settled = false;
    const child = spawn(command, args, {
      cwd: options.cwd ?? process.cwd(),
      env: options.env ?? process.env,
      stdio: ["ignore", "pipe", "pipe"],
    });

    function finish(stage: AcceptanceStage): void {
      if (settled) {
        return;
      }
      settled = true;
      if (stage.ok) {
        logger.info(name, `执行成功：${stage.summary}`);
      } else {
        logger.error(name, `执行失败：${stage.summary}`);
      }
      resolve(stage);
    }

    child.stdout.on("data", (chunk: Buffer) => {
      const text = chunk.toString();
      stdout += text;
      logger.stdout(text);
    });

    child.stderr.on("data", (chunk: Buffer) => {
      const text = chunk.toString();
      stderr += text;
      logger.stderr(text);
    });

    child.on("error", (error) => {
      const summary = truncateSummary(`启动失败：${error.message}`);
      logger.stderr(`${summary}\n`);
      finish({
        name,
        ok: false,
        durationMs: Math.round(performance.now() - startedAt),
        summary,
        logPath: logger.stderrPath,
        exitCode: null,
      });
    });

    child.on("close", (code) => {
      const ok = code === 0;
      const summary = stageSummary(ok, stdout, stderr);
      finish({
        name,
        ok,
        durationMs: Math.round(performance.now() - startedAt),
        summary,
        logPath: ok || !stderr ? logger.stdoutPath : logger.stderrPath,
        exitCode: code,
      });
    });
  });
}
