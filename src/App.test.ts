import { render, screen } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";
import App from "./App.svelte";

describe("App", () => {
  it("renders the main workbench with sidebar and conversation", () => {
    render(App);

    expect(screen.getByTestId("app-shell")).toBeInTheDocument();
    expect(screen.getByTestId("sidebar")).toBeInTheDocument();
    expect(screen.getByTestId("conversation")).toBeInTheDocument();
    expect(screen.getByTestId("task-input")).toBeInTheDocument();
    expect(screen.getByTestId("run-button")).toBeInTheDocument();
  });

  it("does not render the old context panel", () => {
    render(App);
    expect(screen.queryByTestId("context-panel")).not.toBeInTheDocument();
  });
});
