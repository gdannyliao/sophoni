import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { pair, listConversations, getConversation, getWorkspacePath } from "./mobile-api";
import { saveConnection, clearConnection } from "./connection";

// mock global fetch
const fetchMock = vi.fn();
vi.stubGlobal("fetch", fetchMock);

const CONN = { baseUrl: "http://192.168.1.5:43210", token: "test-token" };

beforeEach(() => {
  fetchMock.mockReset();
  saveConnection(CONN);
});

afterEach(() => {
  clearConnection();
});

describe("pair", () => {
  it("成功配对返回 token", async () => {
    fetchMock.mockResolvedValue({
      status: 200,
      ok: true,
      json: async () => ({ token: "new-token" }),
    });

    const result = await pair("http://1.2.3.4:80", "123456");

    expect(fetchMock).toHaveBeenCalledWith("http://1.2.3.4:80/pair", expect.objectContaining({
      method: "POST",
    }));
    expect(result).toEqual({ token: "new-token" });
  });

  it("错误配对码抛 403", async () => {
    fetchMock.mockResolvedValue({ status: 403, ok: false, text: async () => "" });

    await expect(pair("http://1.2.3.4:80", "wrong")).rejects.toThrow("配对码错误");
  });
});

describe("authenticated requests", () => {
  it("listConversations 带 Bearer token", async () => {
    fetchMock.mockResolvedValue({
      status: 200,
      ok: true,
      json: async () => [{ id: "1", title: "会话1", updatedAt: "2026-01-01" }],
    });

    const result = await listConversations();

    expect(fetchMock).toHaveBeenCalledWith(
      "http://192.168.1.5:43210/conversations",
      expect.objectContaining({
        headers: expect.objectContaining({
          Authorization: "Bearer test-token",
        }),
      }),
    );
    expect(result).toHaveLength(1);
  });

  it("401 抛失效错误", async () => {
    fetchMock.mockResolvedValue({ status: 401, ok: false });

    await expect(listConversations()).rejects.toThrow("连接已失效");
  });

  it("getConversation 拼接 id 到路径", async () => {
    fetchMock.mockResolvedValue({
      status: 200,
      ok: true,
      json: async () => ({ id: "abc", title: "x", eventsJson: "[]", updatedAt: "" }),
    });

    await getConversation("abc");

    expect(fetchMock).toHaveBeenCalledWith(
      "http://192.168.1.5:43210/conversations/abc",
      expect.anything(),
    );
  });

  it("getWorkspacePath 返回激活路径", async () => {
    fetchMock.mockResolvedValue({
      status: 200,
      ok: true,
      json: async () => ({ path: "/home/user/project" }),
    });

    const path = await getWorkspacePath();
    expect(path).toBe("/home/user/project");
  });

  it("无连接时抛错", async () => {
    clearConnection();
    await expect(listConversations()).rejects.toThrow("未配对");
  });
});
