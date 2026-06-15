import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { chromium, type Browser, type Page } from "@playwright/test";
import type { AcceptanceLogger } from "./logger";
import type { BrowserAcceptanceResult, BrowserCheck } from "./types";

interface BrowserAcceptanceOptions {
  logger: AcceptanceLogger;
}

const BROWSER_STAGE = "browser";
const APP_URL = "http://127.0.0.1:5173";
const SCREENSHOT_NAME = "browser.png";
const SERVER_TIMEOUT_MS = 30_000;

function check(name: string, ok: boolean, summary?: string): BrowserCheck {
  return { name, ok, ...(summary ? { summary } : {}) };
}

async function waitForServer(url: string, timeoutMs: number): Promise<void> {
  const startedAt = Date.now();
  let lastError = "";

  while (Date.now() - startedAt < timeoutMs) {
    try {
      const response = await fetch(url);
      if (response.ok) {
        return;
      }
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await delay(300);
  }

  throw new Error(`等待 Vite server 超时：${lastError || url}`);
}

function startVite(logger: AcceptanceLogger): ChildProcessWithoutNullStreams {
  logger.info(BROWSER_STAGE, "启动 Vite：pnpm dev --host 127.0.0.1");
  const child = spawn("pnpm", ["dev", "--host", "127.0.0.1"], {
    cwd: process.cwd(),
    env: process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });

  child.stdout.on("data", (chunk: Buffer) => {
    logger.stdout(chunk.toString());
  });
  child.stderr.on("data", (chunk: Buffer) => {
    logger.stderr(chunk.toString());
  });

  return child;
}

async function stopServer(child: ChildProcessWithoutNullStreams, logger: AcceptanceLogger): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }

  logger.info(BROWSER_STAGE, "停止 Vite server");
  child.kill("SIGTERM");

  await new Promise<void>((resolve) => {
    const timeout = setTimeout(() => {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill("SIGKILL");
      }
      resolve();
    }, 5_000);

    child.once("close", () => {
      clearTimeout(timeout);
      resolve();
    });
  });
}

async function hasLocator(page: Page, testId: string): Promise<BrowserCheck> {
  try {
    await page.getByTestId(testId).waitFor({ state: "visible", timeout: 5_000 });
    return check(`${testId} exists`, true);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return check(`${testId} exists`, false, message);
  }
}

export async function runBrowserAcceptance({ logger }: BrowserAcceptanceOptions): Promise<BrowserAcceptanceResult> {
  const screenshotPath = join(logger.runDir, SCREENSHOT_NAME);
  const consoleErrors: string[] = [];
  const checks: BrowserCheck[] = [];
  const server = startVite(logger);
  let browser: Browser | null = null;

  try {
    await waitForServer(APP_URL, SERVER_TIMEOUT_MS);
    logger.info(BROWSER_STAGE, `Vite 可访问：${APP_URL}`);

    browser = await chromium.launch();
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });

    page.on("console", (message) => {
      if (message.type() === "error") {
        consoleErrors.push(message.text());
      }
    });
    page.on("pageerror", (error) => {
      consoleErrors.push(error.message);
    });

    await page.goto(APP_URL, { waitUntil: "networkidle" });

    for (const testId of ["app-shell", "sidebar", "conversation", "context-panel"]) {
      checks.push(await hasLocator(page, testId));
    }

    const taskInput = page.getByTestId("task-input");
    await taskInput.fill("验收：检查运行状态");
    const taskValue = await taskInput.inputValue();
    const taskInputOk = taskValue === "验收：检查运行状态";
    checks.push(check("task input accepts text", taskInputOk, taskInputOk ? undefined : `实际值：${taskValue}`));

    const runButton = page.getByTestId("run-button");
    await runButton.click();
    const stateObserved = await page
      .waitForFunction(() => {
        const button = document.querySelector<HTMLButtonElement>('[data-testid="run-button"]');
        const event = document.querySelector('[data-testid="agent-event"]');
        return Boolean(button?.disabled || event);
      }, undefined, { timeout: 3_000 })
      .then(() => true)
      .catch(() => false);

    checks.push(
      check(
        "run click produces observable state",
        stateObserved,
        stateObserved ? undefined : "点击后未观察到按钮 disabled/running 状态，也未观察到事件或错误输出",
      ),
    );

    await page.screenshot({ path: screenshotPath, fullPage: true });
    checks.push(check("browser screenshot exists", existsSync(screenshotPath), SCREENSHOT_NAME));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logger.error(BROWSER_STAGE, message);
    checks.push(check("browser acceptance completed", false, message));
  } finally {
    if (browser) {
      await browser.close();
    }
    await stopServer(server, logger);
  }

  return {
    url: APP_URL,
    screenshotPath: SCREENSHOT_NAME,
    consoleErrors,
    checks,
  };
}
