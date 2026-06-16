import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { chromium, type Browser, type Page } from "@playwright/test";
import type { AcceptanceLogger } from "./logger";
import type { BrowserAcceptanceResult, BrowserCheck } from "./types";

interface BrowserAcceptanceOptions {
  logger: AcceptanceLogger;
}

interface ViteServer {
  child: ChildProcessWithoutNullStreams;
  port: number;
  url: string;
  ready: boolean;
  stopping: boolean;
  startupFailure: string | null;
  output: string[];
}

interface RunClickObservation {
  beforeEventCount: number;
  beforeButtonEnabled: boolean;
  afterEventCount: number;
  buttonEnteredRunningState: boolean;
}

const BROWSER_STAGE = "browser";
const SCREENSHOT_NAME = "browser.png";
const SERVER_TIMEOUT_MS = 30_000;
const HOST = "127.0.0.1";

function check(name: string, ok: boolean, summary?: string): BrowserCheck {
  return { name, ok, ...(summary ? { summary } : {}) };
}

function outputTail(output: string[]): string {
  return output.join("").trim().split(/\r?\n/).filter(Boolean).slice(-5).join(" | ");
}

function viteFailureMessage(server: ViteServer, reason: string): string {
  const tail = outputTail(server.output);
  return `Vite 启动失败：${reason}${tail ? `；输出：${tail}` : ""}`;
}

export function createViteStartupFailureCheck(summary: string): BrowserCheck {
  return check("vite server starts", false, summary);
}

export function evaluateRunClickObservation(observation: RunClickObservation): BrowserCheck {
  if (!observation.beforeButtonEnabled) {
    return check("run click produces observable state change", false, "点击前 run button 不可用");
  }

  const eventCountIncreased = observation.afterEventCount > observation.beforeEventCount;
  const ok = eventCountIncreased || observation.buttonEnteredRunningState;
  return check(
    "run click produces observable state change",
    ok,
    ok
      ? undefined
      : `点击前事件数 ${observation.beforeEventCount}，点击后事件数 ${observation.afterEventCount}，按钮未进入 disabled/running 状态`,
  );
}

async function getAvailablePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, HOST, () => {
      const address = server.address();
      server.close(() => {
        if (address && typeof address === "object") {
          resolve(address.port);
        } else {
          reject(new Error("无法分配 Vite 端口"));
        }
      });
    });
  });
}

async function waitForServer(server: ViteServer, timeoutMs: number): Promise<void> {
  const startedAt = Date.now();
  let lastError = "";

  while (Date.now() - startedAt < timeoutMs) {
    if (server.startupFailure) {
      throw new Error(server.startupFailure);
    }

    try {
      const response = await fetch(server.url);
      if (response.ok) {
        server.ready = true;
        return;
      }
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await delay(300);
  }

  throw new Error(viteFailureMessage(server, `等待 ${server.url} 超时：${lastError || "无响应"}`));
}

async function startVite(logger: AcceptanceLogger): Promise<ViteServer> {
  const port = await getAvailablePort();
  const url = `http://${HOST}:${port}`;
  logger.info(BROWSER_STAGE, `启动 Vite：pnpm dev --host ${HOST} --port ${port} --strictPort`);
  const child = spawn("pnpm", ["dev", "--host", HOST, "--port", String(port), "--strictPort"], {
    cwd: process.cwd(),
    env: process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const server: ViteServer = {
    child,
    port,
    url,
    ready: false,
    stopping: false,
    startupFailure: null,
    output: [],
  };

  function captureOutput(text: string): void {
    server.output.push(text);
    if (server.output.length > 20) {
      server.output.shift();
    }
  }

  child.stdout.on("data", (chunk: Buffer) => {
    const text = chunk.toString();
    captureOutput(text);
    logger.stdout(text);
  });
  child.stderr.on("data", (chunk: Buffer) => {
    const text = chunk.toString();
    captureOutput(text);
    logger.stderr(text);
  });
  child.on("error", (error) => {
    if (!server.ready && !server.stopping) {
      server.startupFailure = viteFailureMessage(server, error.message);
    }
  });
  child.on("exit", (code, signal) => {
    if (!server.ready && !server.stopping) {
      server.startupFailure = viteFailureMessage(server, `进程退出 code=${code ?? "null"} signal=${signal ?? "null"}`);
    }
  });
  child.on("close", (code, signal) => {
    if (!server.ready && !server.stopping) {
      server.startupFailure = viteFailureMessage(server, `进程退出 code=${code ?? "null"} signal=${signal ?? "null"}`);
    }
  });

  return server;
}

async function stopServer(server: ViteServer, logger: AcceptanceLogger): Promise<void> {
  const { child } = server;
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }

  logger.info(BROWSER_STAGE, "停止 Vite server");
  server.stopping = true;
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
  let appUrl = "";
  let server: ViteServer | null = null;
  let browser: Browser | null = null;

  try {
    server = await startVite(logger);
    appUrl = server.url;
    await waitForServer(server, SERVER_TIMEOUT_MS);
    logger.info(BROWSER_STAGE, `Vite 可访问：${server.url}`);

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

    await page.goto(server.url, { waitUntil: "networkidle" });

    for (const testId of ["app-shell", "sidebar", "conversation"]) {
      checks.push(await hasLocator(page, testId));
    }

    const taskInput = page.getByTestId("task-input");
    await taskInput.fill("验收：检查运行状态");
    const taskValue = await taskInput.inputValue();
    const taskInputOk = taskValue === "验收：检查运行状态";
    checks.push(check("task input accepts text", taskInputOk, taskInputOk ? undefined : `实际值：${taskValue}`));

    const runButton = page.getByTestId("run-button");
    const beforeEventCount = await page.getByTestId("agent-event").count();
    const beforeButtonEnabled = await runButton.isEnabled();
    let buttonEnteredRunningState = false;
    let afterEventCount = beforeEventCount;

    if (beforeButtonEnabled) {
      await runButton.click();
      const observed = await page
        .waitForFunction((initialEventCount) => {
          const button = document.querySelector<HTMLButtonElement>('[data-testid="run-button"]');
          const eventCount = document.querySelectorAll('[data-testid="agent-event"]').length;
          const running = Boolean(button?.disabled || button?.textContent?.includes("运行中"));
          return running || eventCount > initialEventCount ? { eventCount, running } : false;
        }, beforeEventCount, { timeout: 3_000 })
        .then((handle) => handle.jsonValue() as Promise<{ eventCount: number; running: boolean }>)
        .catch(() => null);

      buttonEnteredRunningState = Boolean(observed?.running);
      afterEventCount = observed?.eventCount ?? (await page.getByTestId("agent-event").count());
    }

    checks.push(
      evaluateRunClickObservation({
        beforeEventCount,
        beforeButtonEnabled,
        afterEventCount,
        buttonEnteredRunningState,
      }),
    );

    // 验证设置面板 + 风险等级选择器
    try {
      const settingsButton = page.getByTestId("settings-button");
      await settingsButton.click();
      const settingsPanel = page.getByTestId("settings-panel");
      const panelVisible = await settingsPanel.isVisible().catch(() => false);
      checks.push(check("settings panel opens", panelVisible));

      if (panelVisible) {
        const riskOptions = page.getByTestId("risk-level-options");
        const optionsVisible = await riskOptions.isVisible().catch(() => false);
        checks.push(check("risk level options visible", optionsVisible));

        const standardRadio = page.getByTestId("risk-level-standard");
        const standardVisible = await standardRadio.isVisible().catch(() => false);
        checks.push(check("risk level standard option exists", standardVisible));
      }
    } catch {
      checks.push(check("settings panel opens", false, "设置面板交互失败"));
    }

    // 验证工作区按钮存在
    try {
      const openCount = await page.getByTestId("workspace-open").count().catch(() => 0);
      const switchCount = await page.getByTestId("workspace-switch").count().catch(() => 0);
      checks.push(check("workspace button exists", openCount > 0 || switchCount > 0));
    } catch {
      checks.push(check("workspace button exists", false, "工作区按钮未找到"));
    }

    await page.screenshot({ path: screenshotPath, fullPage: true });
    checks.push(check("browser screenshot exists", existsSync(screenshotPath), SCREENSHOT_NAME));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logger.error(BROWSER_STAGE, message);
    checks.push(
      message.startsWith("Vite 启动失败")
        ? createViteStartupFailureCheck(message)
        : check("browser acceptance completed", false, message),
    );
  } finally {
    if (browser) {
      await browser.close();
    }
    if (server) {
      await stopServer(server, logger);
    }
  }

  return {
    url: appUrl,
    screenshotPath: SCREENSHOT_NAME,
    consoleErrors,
    checks,
  };
}
