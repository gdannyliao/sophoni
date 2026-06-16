import { render, screen } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";
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
    render(Conversation, {
      props: { streamingText: "", summary: "任务已完成", running: false },
    });

    expect(screen.queryByTestId("streaming-bubble")).not.toBeInTheDocument();
    expect(screen.getByText("任务已完成")).toBeInTheDocument();
  });

  it("round_timing 事件渲染成耗时徽章", () => {
    const events = [
      { kind: "round_timing", title: "轮次1", body: "3200ms · 工具调用×2", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    const badge = screen.getByTestId("round-timing");
    expect(badge).toBeInTheDocument();
    expect(badge.textContent).toContain("轮次1");
    expect(badge.textContent).toContain("3200ms");
  });

  it("thought 事件渲染推理文本（body 字段）", () => {
    const events = [
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
      { kind: "round_timing", title: "轮次1", body: "1000ms · 工具调用×1", toolCallId: undefined },
      { kind: "round_timing", title: "轮次2", body: "2000ms · 最终答案", toolCallId: undefined },
    ];
    render(Conversation, { props: { events } });

    const badges = screen.getAllByTestId("round-timing");
    expect(badges).toHaveLength(2);
    expect(badges[0].textContent).toContain("轮次1");
    expect(badges[1].textContent).toContain("轮次2");
  });
});
