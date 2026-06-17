/**
 * 手机端 API 模块。与桌面端 api.ts 平行，函数签名尽量一致，
 * 但底层走 HTTP（连桌面端服务层）而非 Tauri invoke。
 *
 * 桌面端：runAgentTask 是 invoke（一次性返回）+ onAgentEvent（单独 listen 事件流）。
 * 手机端：/chat 是 SSE 流，事件和最终结果在同一个流里。因此 runAgentTask
 * 接收 onEvent 回调，边收 SSE 边推事件，流结束返回最终结果。
 */

import { createParser } from "eventsource-parser";
import type {
  AgentEvent,
  AgentTaskResult,
  ConfigStatus,
  Conversation,
  ConversationSummary,
  RiskLevel,
} from "../types";
import { type Connection, loadConnection } from "./connection";

class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
    this.name = "ApiError";
  }
}

/** 取当前连接，无连接抛错（调用方应先确保已配对）。 */
function requireConnection(): Connection {
  const conn = loadConnection();
  if (!conn) {
    throw new ApiError(401, "未配对，请先扫码连接桌面端");
  }
  return conn;
}

/** 带 token 的 fetch 封装。401 时抛 ApiError（上层据此触发重新配对）。 */
async function authedFetch(
  conn: Connection,
  path: string,
  init?: RequestInit,
): Promise<Response> {
  const resp = await fetch(`${conn.baseUrl}${path}`, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
      ...(init?.headers ?? {}),
    },
  });
  if (resp.status === 401) {
    throw new ApiError(401, "连接已失效，请重新配对");
  }
  if (!resp.ok) {
    const text = await resp.text().catch(() => "");
    throw new ApiError(resp.status, text || `HTTP ${resp.status}`);
  }
  return resp;
}

/** 配对：用二维码扫到的 baseUrl + 配对码换 token。成功后存连接信息。 */
export async function pair(
  baseUrl: string,
  code: string,
): Promise<{ token: string }> {
  const resp = await fetch(`${baseUrl}/pair`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (resp.status === 403) {
    throw new ApiError(403, "配对码错误或已失效");
  }
  if (!resp.ok) {
    throw new ApiError(resp.status, `配对失败: HTTP ${resp.status}`);
  }
  const data = (await resp.json()) as { token: string };
  return data;
}

/** GET /conversations — 当前工作区会话列表。 */
export async function listConversations(): Promise<ConversationSummary[]> {
  const conn = requireConnection();
  const resp = await authedFetch(conn, "/conversations");
  return resp.json();
}

/** GET /conversations/:id — 会话详情（含 events_json + turns_json）。 */
export async function getConversation(id: string): Promise<Conversation> {
  const conn = requireConnection();
  const resp = await authedFetch(conn, `/conversations/${encodeURIComponent(id)}`);
  return resp.json();
}

/** GET /workspaces — 桌面所有工作区。 */
export async function listWorkspaces(): Promise<
  { id: string; name: string; path: string; lastOpenedAt: string }[]
> {
  const conn = requireConnection();
  const resp = await authedFetch(conn, "/workspaces");
  return resp.json();
}

/** GET /workspaces/active — 当前激活工作区路径。 */
export async function getWorkspacePath(): Promise<string | null> {
  const conn = requireConnection();
  const resp = await authedFetch(conn, "/workspaces/active");
  const data = (await resp.json()) as { path: string | null };
  return data.path;
}

/** PUT /workspaces/active — 切换激活工作区。 */
export async function setWorkspacePath(path: string): Promise<void> {
  const conn = requireConnection();
  await authedFetch(conn, "/workspaces/active", {
    method: "PUT",
    body: JSON.stringify({ path }),
  });
}

/** GET /config/risk-level — 当前风险等级。 */
export async function getRiskLevel(): Promise<RiskLevel> {
  const conn = requireConnection();
  const resp = await authedFetch(conn, "/config/risk-level");
  return resp.json();
}

/**
 * POST /chat — 发消息续聊，SSE 流推送 agent 事件。
 *
 * 与桌面端不同：事件不是单独 listen，而是在这个调用的 SSE 响应流里。
 * onEvent 回调每收到一个事件触发（含 token/thought/round_timing/tool_call/tool_result/summary）。
 * 流结束后 resolve 最终的 AgentTaskResult（从最后的 summary 事件提取）。
 *
 * @returns cancel 函数，调用即中止 SSE 流（等价桌面端 cancelAgentTask）。
 */
export function runAgentTask(
  prompt: string,
  existingConversationId: string | null,
  onEvent: (e: AgentEvent) => void,
): { promise: Promise<AgentTaskResult>; cancel: () => void } {
  const conn = requireConnection();
  const controller = new AbortController();

  const promise = (async (): Promise<AgentTaskResult> => {
    const resp = await fetch(`${conn.baseUrl}/chat`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${conn.token}`,
      },
      body: JSON.stringify({
        prompt,
        conversation_id: existingConversationId,
      }),
      signal: controller.signal,
    });

    if (resp.status === 401) {
      throw new ApiError(401, "连接已失效，请重新配对");
    }
    if (!resp.ok) {
      const text = await resp.text().catch(() => "");
      throw new ApiError(resp.status, text || `HTTP ${resp.status}`);
    }
    if (!resp.body) {
      throw new ApiError(500, "SSE 响应无 body");
    }

    // 解析 SSE 流，每个 data 帧是一个 AgentEvent JSON
    const events: AgentEvent[] = [];
    let summary = "";
    const parser = createParser({
      onEvent: (msg) => {
        if (msg.event !== undefined && msg.event !== "message") return;
        if (!msg.data) return;
        try {
          const agentEvent = JSON.parse(msg.data) as AgentEvent;
          events.push(agentEvent);
          onEvent(agentEvent);
          if (agentEvent.kind === "summary") {
            summary = agentEvent.body;
          }
        } catch {
          // 单帧解析失败不中断流，跳过坏帧
        }
      },
    });

    // 手动读 ReadableStream 字节，解码后喂给 parser
    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      parser.feed(decoder.decode(value, { stream: true }));
    }

    return {
      summary,
      events,
      fileChanges: [],
    };
  })();

  const cancel = () => controller.abort();
  return { promise, cancel };
}
