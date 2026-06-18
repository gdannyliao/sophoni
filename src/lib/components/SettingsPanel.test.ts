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
  it("renders provider status when configured", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_config_status") {
        return Promise.resolve({ configured: true, provider: "glm", model: "glm-4.6" });
      }
      return Promise.resolve(null);
    });

    render(SettingsPanel);

    await waitFor(() => {
      expect(screen.getByText("glm")).toBeInTheDocument();
    });
  });
});
