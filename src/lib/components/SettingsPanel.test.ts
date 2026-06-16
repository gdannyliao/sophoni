import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import { invoke } from "@tauri-apps/api/core";
import SettingsPanel from "./SettingsPanel.svelte";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
});

describe("SettingsPanel", () => {
  it("renders three risk level options", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_config_status") {
        return Promise.resolve({ configured: true, provider: "glm", model: "glm-4.6" });
      }
      if (cmd === "get_risk_level") {
        return Promise.resolve("standard");
      }
      return Promise.resolve(null);
    });

    render(SettingsPanel);

    await waitFor(() => {
      expect(screen.getByTestId("risk-level-options")).toBeInTheDocument();
    });

    expect(screen.getByTestId("risk-level-standard")).toBeInTheDocument();
    expect(screen.getByTestId("risk-level-relaxed")).toBeInTheDocument();
    expect(screen.getByTestId("risk-level-unrestricted")).toBeInTheDocument();
  });

  it("defaults to standard risk level", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_config_status") {
        return Promise.resolve({ configured: true, provider: "glm", model: "glm-4.6" });
      }
      if (cmd === "get_risk_level") {
        return Promise.resolve("standard");
      }
      return Promise.resolve(null);
    });

    render(SettingsPanel);

    await waitFor(() => {
      expect((screen.getByTestId("risk-level-standard") as HTMLInputElement).checked).toBe(true);
    });
  });

  it("calls set_risk_level when switching to unrestricted", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_config_status") {
        return Promise.resolve({ configured: true, provider: "glm", model: "glm-4.6" });
      }
      if (cmd === "get_risk_level") {
        return Promise.resolve("standard");
      }
      return Promise.resolve(null);
    });

    render(SettingsPanel);

    await waitFor(() => {
      expect(screen.getByTestId("risk-level-unrestricted")).toBeInTheDocument();
    });

    const radio = screen.getByTestId("risk-level-unrestricted") as HTMLInputElement;
    await fireEvent.click(radio);

    expect(invokeMock).toHaveBeenCalledWith("set_risk_level", { level: "unrestricted" });
  });
});
