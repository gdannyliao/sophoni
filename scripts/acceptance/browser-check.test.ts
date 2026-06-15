import { describe, expect, it } from "vitest";
import { createViteStartupFailureCheck, evaluateRunClickObservation } from "./browser-check";

describe("browser acceptance checks", () => {
  it("reports Vite startup failures explicitly", () => {
    const check = createViteStartupFailureCheck("Vite 启动失败：端口占用");

    expect(check).toEqual({
      name: "vite server starts",
      ok: false,
      summary: "Vite 启动失败：端口占用",
    });
  });

  it("requires run click observation to change after the click", () => {
    expect(
      evaluateRunClickObservation({
        beforeEventCount: 1,
        beforeButtonEnabled: true,
        afterEventCount: 1,
        buttonEnteredRunningState: false,
      }),
    ).toMatchObject({
      name: "run click produces observable state change",
      ok: false,
      summary: "点击前事件数 1，点击后事件数 1，按钮未进入 disabled/running 状态",
    });

    expect(
      evaluateRunClickObservation({
        beforeEventCount: 1,
        beforeButtonEnabled: true,
        afterEventCount: 2,
        buttonEnteredRunningState: false,
      }).ok,
    ).toBe(true);
  });
});
