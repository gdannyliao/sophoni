# 命令执行器设计规格

**日期**:2026-06-15
**关联**:承接所有工具计划。让 Agent 能跑 `cargo test`/`cargo check`/`tsc` 等验证命令,自己确认改对了没。foundation 已有 `command_risk.rs`(风险分类),这是它的执行层。

## 目标

给 Agent 加 `run_command` 工具,让它能在工作区执行安全命令(测试、编译检查、lint 等),从"能改代码"升级成"能验证代码"。

## 非目标

- **不做 High 风险命令执行/确认**。High 命令一律拒绝,不做确认弹框(后续计划)。
- **不做 shell 执行**。直接 exec(argv),不经过 shell,不支持管道/通配符/重定向。
- **不做多命令组合**。一次 run_command 只跑一个命令,不支持 `&&`/`;`/`|`。
- **不做交互式命令**。不支持需要 stdin 输入的命令(如 `npm login`)。
- **不做后台/长任务**。30s 超时,超时杀进程。不跑 dev server / watch 模式。
- **不改 Agent 循环**。和之前工具一样,只改 dispatch + tool_schemas。

## 核心决策

| # | 决策 | 选择 |
|---|------|------|
| 1 | 风险策略 | **Low 静默执行 + High 拒绝**(返回 is_error) |
| 2 | 工作目录 | 固定 workspace_root,Agent 不传 cwd |
| 3 | 执行方式 | **直接 exec**(`Command::new(argv[0]).args(argv[1..])`),不起 shell |
| 4 | 超时 | **30s**,超时杀进程返回超时错误 |
| 5 | stdout 截断 | 前 100 行 / 4000 字符(取先到的) |
| 6 | stderr 截断 | 前 50 行 / 2000 字符 |
| 7 | 白名单匹配 | **前缀匹配**(允许 `cargo test --filter xxx` 带参数) |
| 8 | 安全防护 | classify_command(shell 结构检查)+ 前缀白名单,双层 |

## 架构

### 与现有代码的关系

纯增量。改动集中在:

```
src-tauri/src/core/
├── domain.rs        [改] AgentToolName 加 RunCommand;AgentToolArgs 加 RunCommand 变体
├── command_risk.rs  [改] classify_command 从精确匹配改成前缀匹配 + 扩充白名单
├── tools.rs         [改] dispatch 加分支 + run_command 实现
├── provider.rs      [改] tool_call_to_openai + parse_tool_call 加分支
├── agent.rs         [改] tool_schemas 加 schema + SYSTEM_PROMPT 加引导 + tool_call_event 加分支
├── tests.rs         [改] L1 测试 + command_risk 前缀匹配测试 + 翻译测试
```

### 领域类型扩展(domain.rs)

```rust
pub enum AgentToolName {
    ReadFile,
    WriteFile,
    ListFiles,
    Grep,
    EditFile,
    RunCommand,   // 新增
}

pub enum AgentToolArgs {
    // ... 既有变体 ...
    RunCommand { command: String },   // 新增
}
```

### command_risk.rs 改造

#### 从精确匹配改成前缀匹配

当前 `classify_command` 对白名单用 `exact_low_risk.contains(&normalized.as_str())`。改成前缀匹配:

```rust
let low_risk_prefixes = [
    "ls",
    "rg",
    "git status",
    "git diff",
    "git log",
    "cargo test",
    "cargo check",
    "cargo build",
    "cargo clippy",
    "npm test",
    "npm run build",
    "pnpm test",
    "pnpm build",
    "pnpm check",
    "pnpm install",  // 不加！install 类在 high_risk_markers 里
    "yarn test",
    "tsc",
];
```

注意:`pnpm install`/`npm install`/`cargo install` **不加进白名单**(它们在 `high_risk_markers` 里,是 High)。

匹配逻辑:

```rust
// 先检查 shell 结构(已有)——含 && | $() ; 等的直接 High
if has_shell_structure_risk(&normalized) {
    return CommandRisk::High;
}

// 再检查 high_risk_markers(已有)——含 rm/curl/sudo/git reset 等的 High
// ...

// 改这里:精确匹配 → 前缀匹配
if low_risk_prefixes
    .iter()
    .any(|prefix| normalized == *prefix || normalized.starts_with(&format!("{prefix} ")))
{
    return CommandRisk::Low;
}

CommandRisk::High
```

**关键**:`normalized == *prefix || normalized.starts_with("{prefix} ")`——精确匹配或后面跟空格(确保是独立命令前缀,不是子串)。`cargo` 不会匹配 `cargo_test`(下划线不是空格)。

### 工具实现(tools.rs)

#### run_command 方法

```rust
async fn run_command(&self, call_id: &str, command: &str) -> AppResult<AgentToolResult> {
    // 1. 风险分类
    let risk = classify_command(command, "");
    if risk == CommandRisk::High {
        return Ok(tool_error(call_id, &format!(
            "命令被拒绝(高风险): {command}\n只允许安全的只读命令(cargo test/check/build、git status/diff/log、ls、rg、tsc、pnpm test/build/check 等)。"
        )));
    }

    // 2. 拆 argv(不用 shell)
    let argv = match shell_words(command) {
        v if v.is_empty() => return Ok(tool_error(call_id, "空命令")),
        v => v,
    };

    // 3. 执行(30s 超时)
    let root = self.fs.root().clone();
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&root)
            .output(),
    )
    .await;

    match output {
        Ok(Ok(out)) => {
            let stdout = truncate_output(&String::from_utf8_lossy(&out.stdout), 100, 4000);
            let stderr = truncate_output(&String::from_utf8_lossy(&out.stderr), 50, 2000);
            let exit_code = out.status.code().unwrap_or(-1);
            let content = format!(
                "exit code: {exit_code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
            );
            let is_error = exit_code != 0;
            Ok(AgentToolResult {
                tool_call_id: call_id.to_string(),
                content,
                is_error,
                file_change: None,
            })
        }
        Ok(Err(e)) => Ok(tool_error(call_id, &format!("执行失败: {e}"))),
        Err(_) => Ok(tool_error(call_id, "命令超时(30s),已被终止")),
    }
}
```

**关键设计点**:

1. **exit code 非 0 不抛 Err,返回 is_error: true**。模型需要看到"测试失败"的 stderr 输出来自纠(改代码重试)。如果抛 Err,Agent 循环会把它当系统错误停止。失败 + stderr 让模型自己读错误信息修正。

2. **成功/失败的判断基于 exit code**。exit 0 = is_error: false,exit 非 0 = is_error: true。stderr 有内容但 exit 0 时 is_error: false(很多命令往 stderr 写 warning)。

3. **truncate_output 辅助函数**:

```rust
fn truncate_output(s: &str, max_lines: usize, max_chars: usize) -> String {
    let truncated = s.chars().take(max_chars).collect::<String>();
    let lines: Vec<&str> = truncated.lines().take(max_lines).collect();
    let result = lines.join("\n");
    let total_lines = s.lines().count();
    let total_chars = s.chars().count();
    if total_lines > max_lines || total_chars > max_chars {
        format!("{result}\n（输出已截断，显示前 {}/{} 行。如需完整输出，请在终端手动运行。）", lines.len(), total_lines)
    } else {
        result
    }
}
```

### dispatch 分支

```rust
(AgentToolName::RunCommand, AgentToolArgs::RunCommand { command }) => {
    self.run_command(&call.id, command).await
}
```

### tool_schemas 扩展(agent.rs)

```rust
AgentToolSchema {
    name: "run_command",
    description: "在工作区执行安全命令（测试、编译检查、lint 等）。只允许只读命令：cargo test/check/build/clippy、git status/diff/log、ls、rg、tsc、pnpm test/build/check。命令直接执行，不支持管道、重定向或 shell 特殊字符。",
    parameters: serde_json::json!({
        "type": "object",
        "properties": {
            "command": { "type": "string", "description": "要执行的命令（如 cargo test）" }
        },
        "required": ["command"]
    }),
},
```

### SYSTEM_PROMPT 更新

在可用工具列表加 run_command,在工作方式加验证引导:

```
可用工具：
- list_files：列出目录内容。
- grep：按正则搜索文件内容。
- read_file：读取指定文件内容。
- write_file：写入整个文件。
- edit_file：精确替换文件中的一段文本。
- run_command：执行安全命令（cargo test、git status 等），验证代码改动。

工作方式：
1. 不确定路径时，先 list_files 或 grep 探索。
2. 改文件前，先用 read_file 看当前内容。
3. 小改动优先用 edit_file，大改动或新建文件用 write_file。
4. edit_file 的 old_string 必须与文件内容精确匹配（含缩进和空格）。
5. 当用户要求替换「所有」或「全部」时，用 edit_file 的 replace_all=true。
6. 改完代码后，用 run_command 跑 cargo check 或 cargo test 验证改动是否正确。如果命令失败，读 stderr 定位问题并修正。
7. 不要在回复里直接给文件内容，通过工具操作。
8. 完成任务后给出简短总结。
```

### tool_call_event 扩展(agent.rs)

```rust
AgentToolArgs::RunCommand { command } => {
    ("run_command", command.clone(), format!("command: {command}"))
}
```

### Provider 翻译(provider.rs)

#### tool_call_to_openai(领域 → wire)

```rust
AgentToolArgs::RunCommand { command } => (
    "run_command",
    serde_json::json!({ "command": command }),
),
```

#### parse_tool_call(wire → 领域)

```rust
"run_command" => AgentToolName::RunCommand,
// ...
AgentToolName::RunCommand => {
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Provider("run_command missing command".into()))?
        .to_string();
    AgentToolArgs::RunCommand { command }
}
```

### 依赖(Cargo.toml)

当前 tokio 的 features 是 `["rt-multi-thread", "macros", "time", "sync", "fs"]`,**缺 `process`**。`tokio::process::Command` 需要 `process` feature。改 Cargo.toml:

```toml
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync", "fs", "process"] }
```

只加 `"process"` 一个 feature,其余不变。无需新增 crate。

## 测试策略

### command_risk 前缀匹配测试

1. `cargo test` → Low
2. `cargo test -- --test-name foo` → Low(前缀匹配带参数)
3. `cargo check` → Low(新白名单)
4. `cargo clippy` → Low(新白名单)
5. `tsc --noEmit` → Low(新白名单)
6. `pnpm build` → Low(新白名单)
7. `git log --oneline -5` → Low(新白名单)
8. `cargo test && rm -rf /` → High(shell 结构 `&&`)
9. `rm -rf /` → High(high_risk_markers)
10. `npm install` → High(install 类)
11. `echo hello` → High(不在白名单)

### run_command L1 测试

12. 成功执行 `echo hello`(测试用 echo 作为已知安全命令)→ 注意:echo 不在白名单,需要在测试里用白名单内的命令。**用 `ls` 测试**:执行 `ls` 在临时目录 → stdout 含文件名
13. 执行 `ls` 在临时目录 → stdout 含文件名
14. High 命令 `rm` → is_error,拒绝
15. High 命令含 `&&` → is_error,拒绝
16. 超时:跑一个 sleep 60(需要 mock 或用 `sleep`——但 sleep 不在白名单,High)。**这个测不了**(白名单内没有慢命令)。跳过或用 `cargo test` 在故意失败的测试上(慢但会结束)。
17. exit code 非 0(如 `ls /nonexistent`)→ is_error: true,stderr 有内容

### 翻译测试

18. `parse_tool_call("run_command", {command: "cargo test"})` → 正确的 RunCommand args
19. `parse_tool_call("run_command", {})` 缺 command → Provider 错误

### 不加 L2

循环逻辑不变。

## 成功标准

1. **验证改动**:Agent 改完代码后,自动跑 `cargo check` 或 `cargo test`,看 exit code 判断改对了没。
2. **失败自纠**:Agent 跑 `cargo test` 失败(exit 非 0),读 stderr,定位问题,改代码重试。
3. **安全拒绝**:Agent 尝试 `rm`/`git reset --hard`/`npm install`,全部被拒绝(is_error)。
4. **输出截断**:`cargo test` 输出很长,Agent 收到截断提示,知道信息不全。
5. **测试**:cargo test 全绿(含新增测试)、pnpm check/test/build 全绿。

## 后续计划

- **High 风险确认 UI**:前端弹确认框,用户点同意才执行 High 命令。需要 Tauri 双向通信 + 前端确认组件。
- **可配置白名单**:用户在 config.toml 自定义允许的命令。
- **进程管理**:长任务(dev server/watch)的后台执行 + 日志流。
- **工作区 UI**:让用户选真实工作区,命令执行器在真实项目里跑。
