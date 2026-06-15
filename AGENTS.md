# AGENTS.md

## 开发规范
### 文档规范

1. 文档必须用中文书写，特定的专业词汇可以用英文
2. **实施 plan / task breakdown 文档不入仓**。比如`docs/superpowers/plans/*.md` 


### 提交规范

1. 提交说明必须用中文书写
2. 使用约定式提交格式：`类型(范围): 描述`
   - 类型：`feat`（新功能）、`fix`（修复）、`refactor`（重构）、`chore`（杂项）、`docs`（文档）、`style`（样式）、`test`（测试）
   - 示例：`refactor(api): 提取单词归一化逻辑`、`fix(web): 修复视频播放器暂停问题`

### AI 自测与验收规范

1. 完成代码修改后，AI 必须优先运行固定验收命令：

   ```bash
   pnpm accept
   ```

2. `pnpm accept` 会执行类型检查、前端测试、Rust 测试，并启动 Web 入口做浏览器级 UI 验收。验收产物写入 `.sophoni/runs/<timestamp>/`，该目录不入仓。
3. AI 判断工程状态时，必须优先读取最新 `report.json`：
   - `ok=true` 表示固定验收通过
   - `ok=false` 表示固定验收失败，必须读取 `failureSummary`
   - 不要只凭 stdout 的最后几行判断成功或失败
4. 需要追踪中间状态时，读取同一运行目录下的日志：
   - `events.log`：人类可读的阶段日志
   - `stdout.log` / `stderr.log`：完整命令输出
   - `browser.png`：Web UI 验收截图
5. 如果 `report.json.ok=false`，AI 应先根据 `failureSummary` 定位失败阶段，再读取对应日志继续修复；修复后重新运行 `pnpm accept`。
6. 如果只做很小的局部修改，可以先运行更窄的命令（例如 `pnpm check`、`pnpm test`、`cargo test --manifest-path src-tauri/Cargo.toml`），但最终收尾前仍必须运行 `pnpm accept`。
7. 验收报告和日志用于自测证据。最终回复用户时，应说明运行过的命令、是否通过，以及最新 `report.json` 路径。
