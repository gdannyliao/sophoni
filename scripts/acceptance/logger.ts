import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { AcceptanceEventLevel } from "./types";

export interface AcceptanceLogger {
  runDir: string;
  eventsPath: string;
  stdoutPath: string;
  stderrPath: string;
  info(stage: string, message: string): void;
  error(stage: string, message: string): void;
  stdout(text: string): void;
  stderr(text: string): void;
}

interface LoggerOptions {
  root?: string;
  now?: () => Date;
}

function timestampForDir(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getUTCFullYear()}${pad(date.getUTCMonth() + 1)}${pad(date.getUTCDate())}-${pad(date.getUTCHours())}${pad(date.getUTCMinutes())}${pad(date.getUTCSeconds())}`;
}

function appendEvent(path: string, now: () => Date, level: AcceptanceEventLevel, stage: string, message: string): void {
  appendFileSync(path, `${now().toISOString()} ${level.padEnd(5)} ${stage} ${message}\n`, "utf8");
}

function createUniqueRunDir(root: string, timestamp: string): string {
  const runsDir = join(root, "runs");
  mkdirSync(runsDir, { recursive: true });

  for (let index = 0; ; index += 1) {
    const suffix = index === 0 ? "" : `-${index}`;
    const runDir = join(runsDir, `${timestamp}${suffix}`);
    try {
      mkdirSync(runDir);
      return runDir;
    } catch (error) {
      if (error instanceof Error && "code" in error && error.code === "EEXIST") {
        continue;
      }
      throw error;
    }
  }
}

export function createAcceptanceLogger(options: LoggerOptions = {}): AcceptanceLogger {
  const now = options.now ?? (() => new Date());
  const root = options.root ?? join(process.cwd(), ".sophoni");
  const runDir = createUniqueRunDir(root, timestampForDir(now()));
  const eventsPath = join(runDir, "events.log");
  const stdoutPath = join(runDir, "stdout.log");
  const stderrPath = join(runDir, "stderr.log");

  writeFileSync(eventsPath, "", "utf8");
  writeFileSync(stdoutPath, "", "utf8");
  writeFileSync(stderrPath, "", "utf8");

  return {
    runDir,
    eventsPath,
    stdoutPath,
    stderrPath,
    info(stage, message) {
      appendEvent(eventsPath, now, "INFO", stage, message);
    },
    error(stage, message) {
      appendEvent(eventsPath, now, "ERROR", stage, message);
    },
    stdout(text) {
      appendFileSync(stdoutPath, text, "utf8");
    },
    stderr(text) {
      appendFileSync(stderrPath, text, "utf8");
    },
  };
}
