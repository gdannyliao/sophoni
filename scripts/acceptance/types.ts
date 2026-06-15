export type AcceptanceEventLevel = "INFO" | "ERROR";

export interface AcceptanceStage {
  name: string;
  ok: boolean;
  durationMs: number;
  summary: string;
  logPath: string;
}

export interface BrowserCheck {
  name: string;
  ok: boolean;
  summary?: string;
}

export interface BrowserAcceptanceResult {
  url: string;
  screenshotPath: string;
  consoleErrors: string[];
  checks: BrowserCheck[];
}

export interface AcceptanceReport {
  ok: boolean;
  startedAt: string;
  finishedAt: string;
  runDir: string;
  stages: AcceptanceStage[];
  browser: BrowserAcceptanceResult | null;
  failureSummary: string;
}
