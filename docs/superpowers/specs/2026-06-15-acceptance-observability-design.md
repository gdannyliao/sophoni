# 验收观测层设计

## 背景

当前 Sophoni 已有 Agent 事件流和文件工具调用记录，适合展示模型的思考和文件变更过程。但在一次代码修改完成后，模型还缺少稳定的运行证据来判断工程是否真的可用：它不能可靠启动 Web 入口、读取运行日志、观察 UI 状态，也没有统一的结构化验收结果。

本设计目标是增加一层“验收观测层”，让模型和人都能看见工程运行状态。模型读取结构化报告来判断是否通过，人读取日志、截图和事件流来追踪中间状态。

## 目标

1. 提供固定验收入口，作为每次修改后的最低可判定基线。
2. 生成机器可解析的验收报告，帮助 Agent 自己判断是否通过。
3. 生成人类可读的运行日志，展示命令、启动、浏览器验收和失败摘要。
4. 首版覆盖 Web/Vite UI 级验收，并对 Tauri 桌面入口做构建或启动日志健康检查。
5. 为后续 Agent 工具扩展保留接口，使模型可以读取验收报告、日志和截图路径。

## 非目标

1. 首版不做完整 Tauri 桌面 UI 自动化。
2. 首版不开放任意 shell 执行给模型；如需运行命令，应走受限白名单或固定验收入口。
3. 首版不引入长期后台守护进程。
4. 首版不把验收产物提交入仓。

## 总体方案

采用“固定验收命令 + 结构化报告 + 可读事件日志 + 后续 Agent 工具”的组合方案。

固定验收命令建议命名为 `pnpm accept`。它负责执行稳定基线，包括类型检查、前端测试、Rust 测试、Web 启动检查和浏览器级 UI 验收。每次运行会创建独立运行目录，写入中间日志、最终报告和必要截图。

Agent 后续可以通过工具读取最近一次或指定运行目录的 `report.json` 与 `events.log`，根据结构化字段判断是否通过，并在失败时读取失败阶段对应日志继续修复。

## 运行目录

验收产物写入本地目录：

```text
.sophoni/runs/<timestamp>/
  events.log
  report.json
  stdout.log
  stderr.log
  browser.png
```

`.sophoni/runs/` 应被 Git 忽略。每次验收创建独立目录，避免覆盖历史证据。

## 报告格式

`report.json` 面向模型消费，字段保持稳定：

```json
{
  "ok": false,
  "startedAt": "2026-06-15T00:00:00Z",
  "finishedAt": "2026-06-15T00:00:10Z",
  "runDir": ".sophoni/runs/20260615-000000",
  "stages": [
    {
      "name": "pnpm check",
      "ok": true,
      "durationMs": 1200,
      "summary": "svelte-check found 0 errors and 0 warnings",
      "logPath": "stdout.log"
    }
  ],
  "browser": {
    "url": "http://127.0.0.1:5173",
    "screenshotPath": "browser.png",
    "consoleErrors": [],
    "checks": [
      { "name": "app shell exists", "ok": true },
      { "name": "prompt input accepts text", "ok": true },
      { "name": "run button enters running state", "ok": true }
    ]
  },
  "failureSummary": "pnpm test 失败：src/App.test.ts 中按钮状态断言未通过"
}
```

顶层 `ok` 是模型的首要判断字段。`stages` 保存每个阶段的结果。`failureSummary` 在失败时必须给出短句，帮助人和模型快速定位。

## 事件日志

`events.log` 面向人类阅读，也可以被模型作为补充证据。每行记录一个阶段事件，包含时间、级别、阶段和消息：

```text
2026-06-15T00:00:00Z INFO  accept      创建运行目录 .sophoni/runs/20260615-000000
2026-06-15T00:00:01Z INFO  pnpm-check  开始执行 pnpm check
2026-06-15T00:00:02Z INFO  pnpm-check  通过，0 errors，0 warnings
2026-06-15T00:00:04Z ERROR browser     控制台出现 1 条错误，截图已保存 browser.png
```

事件日志应尽量短而有用，不直接倾倒完整命令输出。完整输出进入 `stdout.log` 和 `stderr.log`。

## Web UI 验收

首版浏览器验收使用 Vite 入口。验收脚本启动 `pnpm dev`，等待本地 URL 可访问，然后执行以下检查：

1. 页面能打开，且没有阻塞性加载错误。
2. 控制台没有 error 级别日志。
3. 三栏主界面存在：侧边栏、对话区、上下文区。
4. 任务输入框可以输入文本。
5. 点击运行按钮后，界面进入运行中状态或产生事件输出。
6. 保存页面截图到 `browser.png`。

这组检查用于确认“应用能被用户操作”。它不是业务正确性的全部证明，后续任务可以在此基础上增加场景化检查。

## Tauri 健康检查

首版不做完整桌面交互自动化。Tauri 侧先执行构建或测试健康检查：

1. `cargo test --manifest-path src-tauri/Cargo.toml`
2. 必要时增加 `pnpm tauri build --debug` 或轻量启动检查
3. 记录构建和启动日志中的错误摘要

这样能覆盖 Rust Core Runtime 和 Tauri 配置的基础健康，不阻塞 Web UI 验收闭环。

## Agent 工具扩展

后续给 Agent 增加只读观测工具：

1. `read_acceptance_report`：读取最近一次或指定运行目录的 `report.json`。
2. `read_runtime_log`：读取指定日志文件，可限制最大行数。
3. `list_acceptance_runs`：列出最近 N 次验收运行。

如果后续需要让 Agent 主动触发验收，可以增加受限工具：

1. `run_acceptance`：只允许执行固定验收入口。
2. `run_safe_command`：只允许执行白名单命令，例如 `pnpm check`、`pnpm test`、`cargo test`。

工具结果应返回结构化摘要，避免模型只能从大段 stdout 中猜测状态。

## 错误处理

1. 任一阶段失败时，验收继续收集可用证据，但最终 `ok=false`。
2. 如果 Web 服务器启动失败，报告中明确标记 `browser` 阶段未执行。
3. 如果浏览器验收超时，保存已有日志并写入失败摘要。
4. 如果截图失败，不应掩盖更早的主要失败原因。
5. 所有路径都应限制在工作区内，避免读取或写入工作区外文件。

## 测试策略

1. 单元测试覆盖报告结构生成、阶段结果合并、失败摘要生成。
2. 集成测试覆盖 `pnpm accept` 在成功和模拟失败时的退出码与报告文件。
3. 浏览器验收脚本覆盖核心 DOM 检查和控制台错误收集。
4. Rust 侧工具扩展测试覆盖路径限制、最近运行选择和日志截断。

## 首版完成标准

1. `pnpm accept` 可运行，并在结束后打印报告路径。
2. 成功时 `report.json.ok=true`，失败时 `ok=false` 且包含 `failureSummary`。
3. `events.log` 能展示人类可读的中间状态。
4. Web 验收能保存截图并记录控制台错误。
5. 验收产物目录不入仓。
6. README 增加简短使用说明。
