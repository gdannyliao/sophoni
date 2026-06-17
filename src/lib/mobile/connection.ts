/**
 * 手机端连接状态管理。
 *
 * 存储桌面端连接信息（baseUrl + token）到 localStorage，Tauri Mobile webview 支持。
 * 配对流程：解析二维码 sophoni://pair?ip=&port=&code= → POST /pair → 拿 token → 存这里。
 */

const STORAGE_KEY = "sophoni-mobile-connection";

export interface Connection {
  /** 桌面端 HTTP 服务地址，如 "http://192.168.1.5:43210" */
  baseUrl: string;
  /** 配对后拿到的长期 token */
  token: string;
}

/**
 * 从二维码内容解析出 baseUrl 和配对码。
 * 二维码格式：sophoni://pair?ip=192.168.1.5&port=43210&code=482910
 */
export function parsePairUrl(qrContent: string): { baseUrl: string; code: string } | null {
  try {
    const url = new URL(qrContent);
    if (url.protocol !== "sophoni:" || url.host !== "pair") {
      return null;
    }
    const ip = url.searchParams.get("ip");
    const port = url.searchParams.get("port");
    const code = url.searchParams.get("code");
    if (!ip || !port || !code) {
      return null;
    }
    return { baseUrl: `http://${ip}:${port}`, code };
  } catch {
    return null;
  }
}

/** 读取已存的连接信息。无则返回 null（需重新配对）。 */
export function loadConnection(): Connection | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const conn = JSON.parse(raw) as Connection;
    if (!conn.baseUrl || !conn.token) return null;
    return conn;
  } catch {
    return null;
  }
}

/** 持久化连接信息。 */
export function saveConnection(conn: Connection): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(conn));
}

/** 清除连接信息（配对失效或用户主动断开）。 */
export function clearConnection(): void {
  localStorage.removeItem(STORAGE_KEY);
}

/** 是否已配对（有存活的连接信息）。 */
export function hasConnection(): boolean {
  return loadConnection() !== null;
}
