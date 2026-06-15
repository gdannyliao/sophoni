# edit_file 工具设计规格

**日期**:2026-06-15
**关联**:承接 `2026-06-15-search-tools-design.md`(搜索工具已完成),给 Agent 加 `edit_file`(search-replace)工具,解决 `write_file` 改大文件烧 token 的问题。

**参考**:Claude Code `FileEditTool` 源码(`/Users/ljm/work/other/claude-code-cli/claude-code-source-code/src/tools/FileEditTool/`),从中学习了匹配策略、引号归一化、唯一性校验、replace_all 参数等实战设计。

## 目标

让 Agent 能**精确编辑文件的一小部分**,而不是用 `write_file` 吐整个文件回来。模型给 `old_string`(要找的文本)+ `new_string`(替换成什么),只传 diff 片段,省 90%+ token。

## 非目标(明确不做)

- **不做缩进容错**。Claude Code 也不做,靠"先 read_file 看准确缩进再 edit"保证匹配。我们的 SYSTEM_PROMPT 已有"改文件前先 read"引导。
- **不做批量编辑**(一次多个 old/new 对)。一次 edit_file 只改一处(或 replace_all 全改)。批量编辑是后续计划。
- **不做 read_state 时间戳强制**。Claude Code 用时间戳强制"必须先 read 才能 edit",我们没这个机制,靠 system prompt 引导。
- **不创建新文件**。文件不存在直接返回 is_error,创建文件用 write_file。
- **不做行尾空白 strip**。Claude Code 给 new_string 做 stripTrailingWhitespace,这版不做,保持简单。
- **不动 write_file**。write_file 保留(整文件覆盖),edit_file 是补充。模型自己选:小改用 edit_file,大改/新建用 write_file。

## 核心决策

| # | 决策 | 选择 | 依据 |
|---|------|------|------|
| 1 | 匹配策略 | **精确匹配 + 引号归一化**(曲引号→直引号) | Claude Code `findActualString` 的做法;挡住模型最常见的非ASCII引号错误 |
| 2 | 唯一性校验 | old_string 必须唯一(除非 replace_all=true) | 防误替换;Claude Code 同样设计 |
| 3 | replace_all 参数 | bool,默认 false。true 时替换所有匹配 | 解决"重命名变量"场景 |
| 4 | 缩进容错 | **不做** | Claude Code 也不做;靠 read_first 工作流保证准确性 |
| 5 | 文件不存在 | 返回 is_error,不创建 | 创建用 write_file,职责分离 |
| 6 | old==new 检测 | 直接返回 is_error | 无意义的调用,Claude Code 两道防线 |
| 7 | 安全防护 | 复用 workspace 边界检查(ensure_inside_root) | 与 read_file/write_file 一致 |

## 引号归一化详解

模型(任何 LLM)偶尔会把直引号 `"'` 错误输出成曲引号 `""''`(typographic quotes,排版用的弯引号,常见于 Word/书籍)。

**归一化逻辑**(参考 Claude Code `normalizeQuotes`):

```rust
fn normalize_quotes(s: &str) -> String {
    s.replace('\u{201C}', "\"")   // " 左双曲 → 直双
     .replace('\u{201D}', "\"")   // " 右双曲 → 直双
     .replace('\u{2018}', "'")    // ' 左单曲 → 直单
     .replace('\u{2019}', "'")    // ' 右单曲 → 直单
}
```

**匹配逻辑**(参考 `findActualString`):

1. 先试精确匹配(`content.contains(old_string)`)。命中就用原 old_string。
2. 精确失败 → 把**两边都**归一化成直引号,再匹配。
3. 归一化后命中 → 返回**文件里实际的那段文本**(保留文件原有引号,不是归一化后的)。替换时用文件实际的文本做匹配,保证写回的内容引号风格不变。

**只做曲→直单向归一化**,不反向(代码文件用直引号是常态,模型输出曲引号是错误)。

## 架构

### 与现有代码的关系

纯增量,Agent 循环逻辑不动。改动集中在:

```
src-tauri/src/core/
├── domain.rs       [改] AgentToolName 加 EditFile 变体;AgentToolArgs 加 Edit 变体
├── tools.rs        [改] dispatch 加分支 + edit_file 实现 + normalize_quotes + find_actual_string
├── provider.rs     [改] tool_call_to_glm + parse_tool_call 加分支
├── agent.rs        [改] tool_schemas 加 schema + tool_call_event 加分支 + SYSTEM_PROMPT 更新
├── tests.rs        [改] L1 测试 + 翻译测试
```

### 领域类型扩展(domain.rs)

```rust
pub enum AgentToolName {
    ReadFile,
    WriteFile,
    ListFiles,
    Grep,
    EditFile,   // 新增
}

pub enum AgentToolArgs {
    Read { path: String },
    Write { path: String, content: String },
    ListFiles { path: Option<String>, recursive: bool },
    Grep { pattern: String, path: Option<String>, include: Option<String> },
    EditFile {   // 新增
        path: String,
        old_string: String,
        new_string: String,
        replace_all: bool,
    },
}
```

### 工具实现(tools.rs)

#### edit_file 方法

```rust
async fn edit_file(
    &self,
    call_id: &str,
    path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> AppResult<AgentToolResult> {
    // 1. old==new 直接拒绝
    if old_string == new_string {
        return Ok(tool_error(call_id, "old_string 和 new_string 相同,无需替换"));
    }

    // 2. 读当前内容(复用 WorkspaceFs,含越界检查)
    let full = self.fs.root().join(path);
    let content = match self.fs.read_text(&full) {
        Ok(c) => c,
        Err(e) => return Ok(tool_error(call_id, &format!("读取失败: {e}"))),
    };

    // 3. 查找匹配(精确 + 引号归一化)
    let actual_old = match find_actual_string(&content, old_string) {
        Some(s) => s,
        None => return Ok(tool_error(call_id, &format!(
            "未找到匹配的文本。请先 read_file 确认当前内容。\n搜索: {old_string}"
        ))),
    };

    // 4. 唯一性校验
    let match_count = content.matches(actual_old.as_str()).count();
    if match_count > 1 && !replace_all {
        return Ok(tool_error(call_id, &format!(
            "找到 {match_count} 处匹配,请提供更多上下文使 old_string 唯一,或设 replace_all=true"
        )));
    }

    // 5. 执行替换
    let updated = if replace_all {
        content.replace(&actual_old, new_string)
    } else {
        content.replacen(&actual_old, new_string, 1)
    };

    // 6. 写入(复用 write_text_with_snapshot,算 diff + 越界检查)
    let write = match self.fs.write_text_with_snapshot(&full, &updated) {
        Ok(w) => w,
        Err(e) => return Ok(tool_error(call_id, &format!("写入失败: {e}"))),
    };

    // 7. 构造 FileChange + 返回
    let change = FileChange {
        id: Uuid::new_v4(),
        task_run_id: Uuid::new_v4(),
        path: path.to_string(),
        kind: ChangeKind::Modified,  // edit_file 只改已有文件,永远是 Modified
        diff: write.diff,
        created_at: Utc::now(),
    };

    let summary = if replace_all {
        format!("已替换 {path} 中全部 {match_count} 处匹配")
    } else {
        format!("已替换 {path} 中的 1 处")
    };

    Ok(AgentToolResult {
        tool_call_id: call_id.to_string(),
        content: summary,
        is_error: false,
        file_change: Some(change),
    })
}
```

#### 辅助函数

```rust
/// 查找文件中匹配 old_string 的实际文本。
/// 先试精确匹配,失败则归一化引号后重试。
/// 返回文件里实际的那段文本(保留原引号风格)。
fn find_actual_string(content: &str, old_string: &str) -> Option<String> {
    // 1. 精确匹配
    if content.contains(old_string) {
        return Some(old_string.to_string());
    }

    // 2. 归一化引号后匹配
    let normalized_search = normalize_quotes(old_string);
    let normalized_content = normalize_quotes(content);

    let idx = normalized_content.find(&normalized_search)?;
    // 返回原文里对应位置的文本(保留原引号)
    Some(content[idx..idx + old_string.len()].to_string())
}

fn normalize_quotes(s: &str) -> String {
    s.replace('\u{201C}', "\"")
     .replace('\u{201D}', "\"")
     .replace('\u{2018}', "'")
     .replace('\u{2019}', "'")
}
```

> **实现注意(字节 vs 字符长度)**:曲引号(`'` `"` `"` `"`)是 3 字节 UTF-8,直引号(`'` `"`)是 1 字节。归一化后 `normalized_content` 比 `content` 短,**字节索引对不上**。归一化分支的正确实现方式:
>
> 1. 把 `content` 和 `old_string` 都按 `char_indices` 转成 `(byte_offset, char)` 序列。
> 2. 归一化 char 序列(曲→直)。
> 3. 在归一化后的 char 序列上找匹配,记录起始和结束的**字符位置**。
> 4. 用字符位置映射回原 `char_indices` 的 byte_offset。
> 5. 用 byte_offset 从原 `content` 切片,得到保留原引号的 actual_old。
>
> 这比 JS 版复杂(Rust 字符串按字节切片,JS 按字符),但是必须正确处理。测试用例 10(引号归一化保留原风格)会覆盖这个路径。

### dispatch 分支

```rust
(AgentToolName::EditFile, AgentToolArgs::EditFile { path, old_string, new_string, replace_all }) => {
    self.edit_file(&call.id, path, old_string, new_string, *replace_all).await
}
```

### Provider 翻译(provider.rs)

#### tool_call_to_glm(领域 → GLM)

```rust
AgentToolArgs::EditFile { path, old_string, new_string, replace_all } => (
    "edit_file",
    serde_json::json!({
        "path": path,
        "old_string": old_string,
        "new_string": new_string,
        "replace_all": replace_all
    }),
),
```

#### parse_tool_call(GLM → 领域)

```rust
"edit_file" => AgentToolName::EditFile,
// ...
AgentToolName::EditFile => {
    let path = args.get("path").and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Provider("edit_file missing path".into()))?.to_string();
    let old_string = args.get("old_string").and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Provider("edit_file missing old_string".into()))?.to_string();
    let new_string = args.get("new_string").and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Provider("edit_file missing new_string".into()))?.to_string();
    let replace_all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
    AgentToolArgs::EditFile { path, old_string, new_string, replace_all }
}
```

### tool_schemas 扩展(agent.rs)

```rust
AgentToolSchema {
    name: "edit_file",
    description: "对已有文件做精确文本替换(search-replace)。先 read_file 看准内容,再给出 old_string(必须与文件内容精确匹配,含缩进)和 new_string。old_string 必须在文件中唯一,除非 replace_all=true。",
    parameters: serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "相对工作区根的文件路径" },
            "old_string": { "type": "string", "description": "要替换的文本(精确匹配)" },
            "new_string": { "type": "string", "description": "替换成的文本(必须与 old_string 不同)" },
            "replace_all": { "type": "boolean", "description": "是否替换所有匹配,默认 false(用于重命名等)" }
        },
        "required": ["path", "old_string", "new_string"]
    }),
},
```

### SYSTEM_PROMPT 更新

在可用工具列表加一行,在工作方式里强调"小改用 edit_file":

```
可用工具：
- list_files：列出目录内容，了解工作区结构。
- grep：按正则搜索文件内容。
- read_file：读取指定文件内容。
- write_file：写入整个文件（新建或大改时用）。
- edit_file：精确替换文件中的一段文本（小改时用,比 write_file 省 token）。

工作方式：
1. 不确定路径时，先 list_files 或 grep 探索。
2. 改文件前，先用 read_file 看当前内容。
3. 小改动优先用 edit_file（给出要替换的原文和新文本），大改动或新建文件用 write_file。
4. edit_file 的 old_string 必须与文件内容精确匹配（含缩进和空格）。
5. 不要在回复里直接给文件内容，通过工具操作。
6. 完成任务后给出简短总结。
```

### tool_call_event 扩展(agent.rs)

```rust
AgentToolArgs::EditFile { path, old_string, new_string, replace_all } => {
    let old_preview = old_string.lines().take(3).collect::<Vec<_>>().join("\n");
    let old_suffix = if old_string.lines().count() > 3 { "\n..." } else { "" };
    (
        "edit_file",
        format!("{} (replace_all={})", path, replace_all),
        format!("path: {path}\nreplace_all: {replace_all}\nold_string:\n{old_preview}{old_suffix}\nnew_string ({} 行):", new_string.lines().count().max(1)),
    )
}
```

> old_string 只显示前 3 行(加 `...` 后缀),避免日志太长。new_string 只显示行数,因为完整内容会在右栏 diff 里看到。

## 测试策略

### L1 工具层单测(tools.rs,真临时目录)

1. **基本替换**:文件含 `old`,edit 成 `new` → 文件更新,diff 正确
2. **多行替换**:old_string 跨多行 → 替换成功
3. **未找到匹配**:old_string 不在文件里 → 返回 is_error,提示 read_file
4. **不唯一(默认)**:文件里 3 处 `old`,replace_all=false → is_error,提示"找到 3 处"
5. **replace_all=true**:文件里 3 处 `old`,replace_all=true → 全部替换,返回"已替换 3 处"
6. **old==new**:old_string 等于 new_string → is_error
7. **文件不存在**:edit 不存在的文件 → is_error
8. **越界路径**:path=`../outside` → is_error(复用 ensure_inside_root)
9. **引号归一化**:文件里是直引号 `"hello"`,old_string 给曲引号 `"hello"` → 匹配成功,替换成功
10. **引号归一化保留原风格**:文件里是曲引号,替换后写回的仍是曲引号(用 actual_old 匹配,不改变文件原有引号)

### 翻译函数单测(provider.rs)

11. `parse_tool_call("edit_file", {path, old_string, new_string})` → 正确的 EditFile args
12. `parse_tool_call("edit_file", {path, old_string, new_string, replace_all: true})` → replace_all=true
13. `parse_tool_call("edit_file", {})` 缺必填字段 → 返回 Provider 错误

### 不加 L2

循环逻辑不变,现有 4 个 L2 测试已覆盖。

## 成功标准(验收清单)

1. **小改省 token**:输入"把 README.md 第一行改成 XXX",Agent 调 edit_file(不调 write_file),只传 old_string + new_string,不传整个文件。
2. **精确匹配**:Agent 先 read_file,再 edit_file,old_string 与文件精确匹配,替换成功。
3. **唯一性保护**:Agent 给的 old_string 在文件里多处出现,edit 返回 is_error,Agent 重试用更长的上下文。
4. **重命名**:Agent 用 replace_all=true 把文件里所有 `foo` 改成 `bar`。
5. **引号归一化**:文件用直引号,Agent 偶尔输出曲引号,edit 仍能匹配成功(不报错)。
6. **测试**:cargo test 全绿(含 13 个新测试)、pnpm check/test/build 全绿。

## 后续计划(明确不做)

- **批量编辑**(一次多个 old/new 对):后续计划。
- **缩进容错**:如果实际使用中匹配失败率太高,再加行首空白归一化。
- **read_state 强制**:记录 read_file 的时间戳,edit 时校验"必须先读",像 Claude Code 那样。
- **命令执行器**:让 Agent 能跑 cargo check / git commit。优先级高于批量编辑。
