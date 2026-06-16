import { render, screen } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";
import App from "./App.svelte";

describe("App", () => {
  it("renders welcome view when no active conversation", () => {
    render(App);

    expect(screen.getByTestId("app-shell")).toBeInTheDocument();
    expect(screen.getByTestId("sidebar")).toBeInTheDocument();
    expect(screen.getByTestId("welcome-view")).toBeInTheDocument();
    expect(screen.getByTestId("welcome-input")).toBeInTheDocument();
  });

  it("does not render the old context panel", () => {
    render(App);
    expect(screen.queryByTestId("context-panel")).not.toBeInTheDocument();
  });
});
