import { existsSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { createAcceptanceLogger } from "./logger";

const roots: string[] = [];

afterEach(() => {
  for (const root of roots) {
    rmSync(root, { recursive: true, force: true });
  }
  roots.length = 0;
});

describe("acceptance logger", () => {
  it("creates a timestamped run directory and writes readable events", () => {
    const root = join(process.cwd(), ".tmp-acceptance-logger");
    roots.push(root);

    const logger = createAcceptanceLogger({ root, now: () => new Date("2026-06-15T00:00:00.000Z") });
    logger.info("accept", "创建运行目录");
    logger.error("browser", "控制台出现错误");

    const events = readFileSync(join(logger.runDir, "events.log"), "utf8");

    expect(logger.runDir.endsWith(".tmp-acceptance-logger/runs/20260615-000000")).toBe(true);
    expect(existsSync(join(logger.runDir, "stdout.log"))).toBe(true);
    expect(existsSync(join(logger.runDir, "stderr.log"))).toBe(true);
    expect(events).toContain("2026-06-15T00:00:00.000Z INFO  accept 创建运行目录");
    expect(events).toContain("2026-06-15T00:00:00.000Z ERROR browser 控制台出现错误");
  });

  it("appends command output to stdout and stderr logs", () => {
    const root = join(process.cwd(), ".tmp-acceptance-output");
    roots.push(root);

    const logger = createAcceptanceLogger({ root, now: () => new Date("2026-06-15T00:00:00.000Z") });
    logger.stdout("hello stdout\n");
    logger.stderr("hello stderr\n");

    expect(readFileSync(join(logger.runDir, "stdout.log"), "utf8")).toContain("hello stdout");
    expect(readFileSync(join(logger.runDir, "stderr.log"), "utf8")).toContain("hello stderr");
  });
});
