import { beforeEach, describe, expect, it } from "vitest";
import {
  parsePairUrl,
  loadConnection,
  saveConnection,
  clearConnection,
  hasConnection,
} from "./connection";

beforeEach(() => {
  localStorage.clear();
});

describe("parsePairUrl", () => {
  it("解析合法的 sophoni://pair 二维码内容", () => {
    const result = parsePairUrl("sophoni://pair?ip=192.168.1.5&port=43210&code=482910");
    expect(result).toEqual({
      baseUrl: "http://192.168.1.5:43210",
      code: "482910",
    });
  });

  it("拒绝非 sophoni 协议", () => {
    expect(parsePairUrl("https://example.com/pair?code=123")).toBeNull();
  });

  it("拒绝缺少字段的 URL", () => {
    expect(parsePairUrl("sophoni://pair?ip=1.2.3.4&port=80")).toBeNull();
    expect(parsePairUrl("sophoni://pair?code=123")).toBeNull();
  });

  it("拒绝非法 URL", () => {
    expect(parsePairUrl("not a url")).toBeNull();
  });
});

describe("connection storage", () => {
  it("无连接时 loadConnection 返回 null", () => {
    expect(loadConnection()).toBeNull();
    expect(hasConnection()).toBe(false);
  });

  it("saveConnection 后能 loadConnection 读回", () => {
    const conn = { baseUrl: "http://1.2.3.4:80", token: "abc123" };
    saveConnection(conn);
    expect(hasConnection()).toBe(true);
    expect(loadConnection()).toEqual(conn);
  });

  it("clearConnection 清除存储", () => {
    saveConnection({ baseUrl: "http://1.2.3.4:80", token: "abc" });
    clearConnection();
    expect(loadConnection()).toBeNull();
    expect(hasConnection()).toBe(false);
  });

  it("损坏的存储数据返回 null", () => {
    localStorage.setItem("sophoni-mobile-connection", "{not json");
    expect(loadConnection()).toBeNull();
  });

  it("缺字段的存储数据返回 null", () => {
    localStorage.setItem("sophoni-mobile-connection", JSON.stringify({ baseUrl: "x" }));
    expect(loadConnection()).toBeNull();
  });
});
