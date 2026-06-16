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
});
