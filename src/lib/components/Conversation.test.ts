import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";

vi.mock("../api", () => ({
  getRiskLevel: vi.fn().mockResolvedValue("standard"),
  setRiskLevel: vi.fn().mockResolvedValue(undefined),
}));
import Conversation from "./Conversation.svelte";

describe("Conversation", () => {
  it("流式输出 streamingText 非空时渲染流式气泡", () => {
    render(Conversation, { props: { streamingText: "正在读取", running: true } });

    const bubble = screen.getByTestId("streaming-bubble");
    expect(bubble).toBeInTheDocument();
    expect(bubble.textContent).toContain("正在读取");
  });

  it("streamingText 为空时不渲染流式气泡", () => {
    render(Conversation, { props: { streamingText: "", running: false } });

    expect(screen.queryByTestId("streaming-bubble")).not.toBeInTheDocument();
  });

  it("流式气泡渲染完整累积文本", () => {
    // 模拟多次 token 累积后传给组件的完整字符串
    render(Conversation, {
      props: { streamingText: "读 README\n加注释\n完成", running: true },
    });

    const bubble = screen.getByTestId("streaming-bubble");
    expect(bubble.textContent).toContain("读 README");
    expect(bubble.textContent).toContain("加注释");
    expect(bubble.textContent).toContain("完成");
  });

  it("任务结束有 summary 时不显示流式气泡（由 summary 卡片定型）", () => {
    const events = [
      { kind: "user", title: "用户", body: "做什么", toolCallId: undefined },
      { kind: "summary", title: "任务完成", body: "任务已完成", toolCallId: undefined },
    ];
    render(Conversation, {
      props: { events, streamingText: "", running: false },
    });

    expect(screen.queryByTestId("streaming-bubble")).not.toBeInTheDocument();
    expect(screen.getByText("任务已完成")).toBeInTheDocument();
  });

  it("summary 事件渲染成结果摘要卡片", () => {
    const events = [
      { kind: "user", title: "用户", body: "做什么", toolCallId: undefined },
      { kind: "summary", title: "任务完成", body: "## 完成\n代码已更新", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    const card = screen.getByTestId("summary-card");
    expect(card).toBeInTheDocument();
    expect(card.textContent).toContain("结果摘要");
    expect(card.textContent).toContain("代码已更新");
  });

  it("多轮对话的多条 summary 都保留在流里（不被冲掉）", () => {
    const events = [
      { kind: "user", title: "用户", body: "第一个问题", toolCallId: undefined },
      { kind: "summary", title: "任务完成", body: "第一个答案", toolCallId: undefined },
      { kind: "user", title: "用户", body: "第二个问题", toolCallId: undefined },
      { kind: "summary", title: "任务完成", body: "第二个答案", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    const cards = screen.getAllByTestId("summary-card");
    expect(cards).toHaveLength(2);
    expect(cards[0].textContent).toContain("第一个答案");
    expect(cards[1].textContent).toContain("第二个答案");
    // 两条用户消息也都保留
    const bubbles = screen.getAllByTestId("message-bubble");
    expect(bubbles).toHaveLength(2);
  });

  it("进行中的轮次（无 summary）中间过程展开显示", () => {
    const events = [
      { kind: "user", title: "用户", body: "帮我跑测试", toolCallId: undefined },
      { kind: "round_timing", title: "轮次1", body: "3200ms · 工具调用×2", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    // 进行中：中间过程可见
    const badge = screen.getByTestId("round-timing");
    expect(badge).toBeInTheDocument();
    expect(badge.textContent).toContain("轮次1");
    // 进行中：没有折叠控件（summary 还没到）
    expect(screen.queryByTestId("collapse-toggle")).not.toBeInTheDocument();
  });

  it("thought 事件渲染推理文本（body 字段）", () => {
    const events = [
      { kind: "user", title: "用户", body: "做什么", toolCallId: undefined },
      { kind: "thought", title: "推理", body: "我需要先读取文件看看内容", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    const thought = screen.getByTestId("thought-line");
    expect(thought).toBeInTheDocument();
    // thought 展示用 body（推理文本），不是固定的 title "推理"
    expect(thought.textContent).toContain("我需要先读取文件看看内容");
  });

  it("多个 round_timing 按顺序渲染", () => {
    const events = [
      { kind: "user", title: "用户", body: "做什么", toolCallId: undefined },
      { kind: "round_timing", title: "轮次1", body: "1000ms · 工具调用×1", toolCallId: undefined },
      { kind: "round_timing", title: "轮次2", body: "2000ms · 最终答案", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    const badges = screen.getAllByTestId("round-timing");
    expect(badges).toHaveLength(2);
    expect(badges[0].textContent).toContain("轮次1");
    expect(badges[1].textContent).toContain("轮次2");
  });

  it("user 事件渲染成用户消息气泡", () => {
    const events = [
      { kind: "user", title: "用户", body: "帮我读一下 README", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    const bubble = screen.getByTestId("message-bubble");
    expect(bubble).toBeInTheDocument();
    expect(bubble.textContent).toContain("帮我读一下 README");
  });

  it("连续会话多条 user 消息按顺序渲染", () => {
    const events = [
      { kind: "user", title: "用户", body: "第一条消息", toolCallId: undefined },
      { kind: "user", title: "用户", body: "第二条消息", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    const bubbles = screen.getAllByTestId("message-bubble");
    expect(bubbles).toHaveLength(2);
    expect(bubbles[0].textContent).toContain("第一条消息");
    expect(bubbles[1].textContent).toContain("第二条消息");
  });

  // ── 折叠行为测试 ──

  it("summary 到达后中间过程默认折叠", () => {
    const events = [
      { kind: "user", title: "用户", body: "帮我跑测试", toolCallId: undefined },
      { kind: "round_timing", title: "轮次1", body: "1000ms · 工具调用×1", toolCallId: undefined },
      { kind: "thought", title: "推理", body: "先看看文件", toolCallId: undefined },
      { kind: "summary", title: "任务完成", body: "测试通过", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    // summary 到达：中间过程被折叠（不渲染）
    expect(screen.queryByTestId("turn-process")).not.toBeInTheDocument();
    expect(screen.queryByTestId("round-timing")).not.toBeInTheDocument();
    // 但 summary 卡片和折叠控件始终可见
    expect(screen.getByTestId("summary-card")).toBeInTheDocument();
    const toggle = screen.getByTestId("collapse-toggle");
    expect(toggle.textContent).toContain("已执行 2 步");
    expect(toggle.textContent).toContain("展开");
  });

  it("点击折叠控件展开中间过程", async () => {
    const events = [
      { kind: "user", title: "用户", body: "帮我跑测试", toolCallId: undefined },
      { kind: "round_timing", title: "轮次1", body: "1000ms · 工具调用×1", toolCallId: undefined },
      { kind: "summary", title: "任务完成", body: "测试通过", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    // 初始折叠
    expect(screen.queryByTestId("round-timing")).not.toBeInTheDocument();
    const toggle = screen.getByTestId("collapse-toggle");
    expect(toggle.textContent).toContain("展开");

    // 点击展开
    await fireEvent.click(toggle);
    expect(screen.getByTestId("turn-process")).toBeInTheDocument();
    expect(screen.getByTestId("round-timing")).toBeInTheDocument();
    const toggleAfter = screen.getByTestId("collapse-toggle");
    expect(toggleAfter.textContent).toContain("收起");

    // 再次点击收起
    await fireEvent.click(toggleAfter);
    expect(screen.queryByTestId("round-timing")).not.toBeInTheDocument();
  });

  it("多轮各自独立折叠互不影响", () => {
    const events = [
      { kind: "user", title: "用户", body: "问题1", toolCallId: undefined },
      { kind: "round_timing", title: "轮次1", body: "100ms", toolCallId: undefined },
      { kind: "summary", title: "任务完成", body: "答案1", toolCallId: undefined },
      { kind: "user", title: "用户", body: "问题2", toolCallId: undefined },
      { kind: "round_timing", title: "轮次2", body: "200ms", toolCallId: undefined },
      { kind: "summary", title: "任务完成", body: "答案2", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    // 两轮都默认折叠，各自一个折叠控件
    const toggles = screen.getAllByTestId("collapse-toggle");
    expect(toggles).toHaveLength(2);
    // 中间过程都不可见
    expect(screen.queryAllByTestId("round-timing")).toHaveLength(0);
    // 两个 summary 都可见
    const cards = screen.getAllByTestId("summary-card");
    expect(cards).toHaveLength(2);
  });

  it("出错的轮次不折叠中间过程", () => {
    const events = [
      { kind: "user", title: "用户", body: "做什么", toolCallId: undefined },
      { kind: "round_timing", title: "轮次1", body: "1000ms · 工具调用×1", toolCallId: undefined },
      { kind: "error", title: "错误", body: "命令执行失败", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    // 出错：中间过程保持可见（错误是重要信息，不折叠）
    expect(screen.getByTestId("round-timing")).toBeInTheDocument();
    expect(screen.getByText("命令执行失败")).toBeInTheDocument();
    // 出错轮次不显示折叠控件
    expect(screen.queryByTestId("collapse-toggle")).not.toBeInTheDocument();
  });

  // ── 只读工具调用展示测试 ──

  it("read_file 工具调用渲染成卡片", () => {
    const events = [
      { kind: "user", title: "用户", body: "读 README", toolCallId: undefined },
      { kind: "tool_call", title: "read_file: README.md", body: "path: README.md", toolCallId: "call_1" },
      { kind: "tool_result", title: "结果: call_1", body: "# Sophoni\n一个 Agent", toolCallId: "call_1" },
    ];
    render(Conversation, { props: { events } });

    // 卡片渲染，头部含工具名
    const card = screen.getByTestId("tool-card");
    expect(card).toBeInTheDocument();
    expect(card.textContent).toContain("read_file: README.md");
  });

  it("只读工具卡片展开显示完整结果", async () => {
    const events = [
      { kind: "user", title: "用户", body: "读 README", toolCallId: undefined },
      { kind: "tool_call", title: "read_file: README.md", body: "path: README.md", toolCallId: "call_1" },
      { kind: "tool_result", title: "结果: call_1", body: "# Sophoni\n一个 Agent", toolCallId: "call_1" },
    ];
    render(Conversation, { props: { events } });

    const card = screen.getByTestId("tool-card");
    // 初始折叠：结果不可见
    expect(card.textContent).not.toContain("# Sophoni");
    // 点击展开
    await fireEvent.click(card.querySelector(".tool-header")!);
    expect(card.textContent).toContain("# Sophoni");
    expect(card.textContent).toContain("一个 Agent");
  });

  it("list_files / grep 等只读工具都渲染成卡片", () => {
    const events = [
      { kind: "user", title: "用户", body: "探索", toolCallId: undefined },
      { kind: "tool_call", title: "list_files: src (recursive=false)", body: "path: src", toolCallId: "c1" },
      { kind: "tool_result", title: "结果: c1", body: "dir  lib\nfile  App.svelte", toolCallId: "c1" },
      { kind: "tool_call", title: "grep: /TODO/ in src", body: "pattern: TODO", toolCallId: "c2" },
      { kind: "tool_result", title: "结果: c2", body: "App.svelte:1: TODO", toolCallId: "c2" },
    ];
    render(Conversation, { props: { events } });

    const cards = screen.getAllByTestId("tool-card");
    expect(cards).toHaveLength(2);
    expect(cards[0].textContent).toContain("list_files");
    expect(cards[1].textContent).toContain("grep");
  });

  it("只读工具失败时显示错误样式", async () => {
    const events = [
      { kind: "user", title: "用户", body: "读文件", toolCallId: undefined },
      { kind: "tool_call", title: "read_file: 不存在.txt", body: "path: 不存在.txt", toolCallId: "c1" },
      { kind: "tool_result", title: "结果: c1", body: "失败: 文件不存在", toolCallId: "c1" },
    ];
    render(Conversation, { props: { events } });

    const card = screen.getByTestId("tool-card");
    // 错误图标存在
    expect(card.textContent).toContain("✗");
    // 展开后看到错误信息
    await fireEvent.click(card.querySelector(".tool-header")!);
    expect(card.textContent).toContain("文件不存在");
  });

  it("只读工具运行中（无 tool_result）显示 pending 状态", () => {
    const events = [
      { kind: "user", title: "用户", body: "读文件", toolCallId: undefined },
      { kind: "tool_call", title: "read_file: big.txt", body: "path: big.txt", toolCallId: "c1" },
      // 无 tool_result，模拟运行中
    ];
    render(Conversation, { props: { events } });

    const card = screen.getByTestId("tool-card");
    expect(card.textContent).toContain("⏳");
    expect(card.textContent).toContain("运行中...");
  });

  it("summary 到达后只读工具卡片随中间过程一起折叠", () => {
    const events = [
      { kind: "user", title: "用户", body: "读文件", toolCallId: undefined },
      { kind: "tool_call", title: "read_file: README.md", body: "path: README.md", toolCallId: "c1" },
      { kind: "tool_result", title: "结果: c1", body: "文件内容", toolCallId: "c1" },
      { kind: "summary", title: "任务完成", body: "读取完成", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    // summary 到达：工具卡片随中间过程折叠
    expect(screen.queryByTestId("tool-card")).not.toBeInTheDocument();
    // 但 summary 可见，折叠控件显示步数（含 tool_read 1 步）
    expect(screen.getByTestId("summary-card")).toBeInTheDocument();
    expect(screen.getByTestId("collapse-toggle").textContent).toContain("已执行 1 步");
    });
});

// ── 风险等级选择器（仅桌面端选了工作区时显示）──
describe("Conversation 风险等级", () => {
  it("选了工作区时显示权限选择器", async () => {
    render(Conversation, { props: { workspacePath: "/tmp/test", mobile: false } });
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.getByTestId("risk-bar")).toBeInTheDocument();
    expect(screen.getByTestId("risk-pill-standard")).toBeInTheDocument();
    expect(screen.getByTestId("risk-pill-relaxed")).toBeInTheDocument();
    expect(screen.getByTestId("risk-pill-unrestricted")).toBeInTheDocument();
  });

  it("移动端不显示风险等级", () => {
    render(Conversation, { props: { workspacePath: "移动端", mobile: true } });
    expect(screen.queryByTestId("risk-bar")).not.toBeInTheDocument();
  });

  it("无工作区时不显示风险等级", () => {
    render(Conversation, { props: { workspacePath: "", mobile: false } });
    expect(screen.queryByTestId("risk-bar")).not.toBeInTheDocument();
  });
});
