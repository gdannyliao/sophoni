import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import ConfirmDialog from "./ConfirmDialog.svelte";
import type { CommandConfirmRequest } from "../types";

const mockRequest: CommandConfirmRequest = {
  requestId: "req-1",
  command: "rm src/old.txt",
  reason: "高风险命令",
};

describe("ConfirmDialog", () => {
  it("displays command and reason", () => {
    render(ConfirmDialog, { request: mockRequest });

    expect(screen.getByTestId("confirm-command").textContent).toBe("rm src/old.txt");
    expect(screen.getByText("高风险命令")).toBeInTheDocument();
  });

  it("calls onResolve(true) when allow button clicked", async () => {
    const onResolve = vi.fn();
    render(ConfirmDialog, { request: mockRequest, onResolve });

    await fireEvent.click(screen.getByTestId("confirm-allow"));

    expect(onResolve).toHaveBeenCalledWith(true);
  });

  it("calls onResolve(false) when deny button clicked", async () => {
    const onResolve = vi.fn();
    render(ConfirmDialog, { request: mockRequest, onResolve });

    await fireEvent.click(screen.getByTestId("confirm-deny"));

    expect(onResolve).toHaveBeenCalledWith(false);
  });

  it("calls onResolve(false) when overlay clicked", async () => {
    const onResolve = vi.fn();
    render(ConfirmDialog, { request: mockRequest, onResolve });

    const overlay = document.querySelector(".overlay") as HTMLElement;
    await fireEvent.click(overlay);

    expect(onResolve).toHaveBeenCalledWith(false);
  });
});
