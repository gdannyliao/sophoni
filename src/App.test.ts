import { render, screen } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";
import App from "./App.svelte";

describe("App", () => {
  it("renders the three-column desktop workbench", () => {
    render(App);

    expect(screen.getByRole("complementary", { name: "工作区与会话" })).toBeInTheDocument();
    expect(screen.getByRole("main", { name: "任务会话流" })).toBeInTheDocument();
    expect(screen.getByRole("complementary", { name: "上下文与产物" })).toBeInTheDocument();
    expect(screen.getByTestId("app-shell")).toBeInTheDocument();
    expect(screen.getByTestId("sidebar")).toBeInTheDocument();
    expect(screen.getByTestId("conversation")).toBeInTheDocument();
    expect(screen.getByTestId("context-panel")).toBeInTheDocument();
    expect(screen.getByTestId("task-input")).toBeInTheDocument();
    expect(screen.getByTestId("run-button")).toBeInTheDocument();
  });
});
