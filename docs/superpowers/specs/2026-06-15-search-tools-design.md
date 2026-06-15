# 搜索工具设计规格

**日期**:2026-06-15
**关联**:承接 `2026-06-14-glm-agent-loop-design.md`(GLM Agent 循环已完成并验收),给 Agent 加 `list_files` + `grep` 两个专用搜索工具,让它从「只能改用户指定路径」变成「能自己找文件、找内容」。

## 目标

让 Agent 具备工作区探索能力:

- **`list_files`**:列目录内容,让 Agent 了解工作区结构(现在它连有哪些文件都不知道)。
- **`grep`**:按内容搜索,让 Agent 能找到"所有用了 `invoke` 的地方"这类需求。

两个工具都用纯 Rust 实现,不依赖外部命令(ripgrep 等),后续可升级到 rg 后端。

## 非目标(明确不做)

- **不做命令执行器**。Agent 不能跑 `cargo test`/`git status` 等任意命令。命令执行器是独立计划,涉及风险确认 UI、超时、输出截断等,范围更大。
- **不做完整 .gitignore 解析**。忽略目录用硬编码黑名单覆盖常见场景,完整 gitignore 语法(negation `!`、`**`、嵌套)留后续。
- **不做 rg 后端**。这一版纯 Rust(walkdir + regex),接口预留 rg 升级但不实现。
- **不动 Agent 循环逻辑**。循环已工具无关,只改 `ToolDispatcher::dispatch` 的 match 分支 + `tool_schemas()`。
- **不做结果分页**。超上限直接截断 + 提示,不做"下一页"机制(避免 Agent 反复翻页烧轮次)。
- **不做匹配行上下文**。grep 只返回匹配行,不带前后文(需要上下文时 Agent 自己调 read_file)。

## 核心决策(对话中确认)

| # | 决策 | 选择 |
|---|------|------|
| 1 | 工具集 | `list_files` + `grep` 两个专用工具,不做命令执行器 |
| 2 | 实现路线 | **纯 Rust**(walkdir + regex crate),零外部依赖,预留 rg 升级口 |
| 3 | 结果截断 | grep 最多 100 条匹配,list_files 最多 200 个路径;超出截断 + 提示 |
| 4 | 忽略目录 | 硬编码黑名单(`.git`/`node_modules`/`target`/`dist`/`build`/`.next`/`.svelte-kit`/`__pycache__`),不做完整 gitignore |
| 5 | 大文件跳过 | grep 跳过 >1MB 文件(与 `WorkspaceFs::read_text` 的限制一致) |
| 6 | list_files 参数 | `path`(可选,默认 `.`)+ `recursive`(bool,默认 false) |
| 7 | grep 参数 | `pattern`(必填,正则)+ `path`(可选,默认 `.`)+ `include`(可选,glob 如 `*.ts`) |
| 8 | 结果格式 | **人类可读文本**,模仿 `grep -n` 输出(`path:line:content`),不用 JSON |
| 9 | 安全防护 | 三层:路径词法校验 + 不跟随 symlink(walkdir 默认)+ 深度上限 10 |
| 10 | 测试 | 真临时目录(与 foundation 一致)+ 16 个 L1 用例 + 2 个翻译用例,不加 L2 |
| 11 | SYSTEM_PROMPT | 更新常量,列出全部 4 个工具 + 引导"先搜再改"工作流,仍硬编码不可配 |

## 架构

### 与现有代码的关系

这一版是**纯增量**,不动 Agent 循环逻辑。改动集中在两处:

```
src-tauri/src/core/
├── tools.rs        [改] dispatch 加 2 个分支 + list_files/grep 实现
├── domain.rs       [改] AgentToolName/AgentToolArgs 加 2 个变体
├── provider.rs     [改] tool_call_to_glm + parse_tool_call 加 2 个工具的翻译
├── agent.rs        [改] tool_schemas() 加 2 个 schema 描述(循环逻辑不动)
├── tests.rs        [改] 加 16 个 L1 + 2 个翻译测试
└── Cargo.toml      [改] 加 regex 依赖
```

模块依赖方向不变(无环):

```
agent → tools → workspace → domain
              → regex(新)
provider → domain
```

### 领域类型扩展(domain.rs)

`AgentToolName` 加两个变体:

```rust
pub enum AgentToolName {
    ReadFile,
    WriteFile,
    ListFiles,   // 新增
    Grep,        // 新增
}
```

`AgentToolArgs` 加两个变体:

```rust
pub enum AgentToolArgs {
    Read { path: String },
    Write { path: String, content: String },
    ListFiles { path: Option<String>, recursive: bool },   // 新增
    Grep { pattern: String, path: Option<String>, include: Option<String> },  // 新增
}
```

**既有类型不改**:`AgentToolCall`/`AgentToolResult`/`ConversationTurn` 等原样复用。list_files/grep 的 `AgentToolResult.file_change` 永远是 `None`(只读工具)。

### 工具实现(tools.rs)

`ToolDispatcher` 持有 `WorkspaceFs`(已有),新增方法:

```rust
impl ToolDispatcher {
    pub async fn dispatch(&self, call: &AgentToolCall) -> AppResult<AgentToolResult> {
        match (&call.name, &call.arguments) {
            (AgentToolName::ReadFile, AgentToolArgs::Read { path }) => self.read_file(&call.id, path).await,
            (AgentToolName::WriteFile, AgentToolArgs::Write { path, content }) => self.write_file(&call.id, path, content).await,
            (AgentToolName::ListFiles, AgentToolArgs::ListFiles { path, recursive }) => {
                self.list_files(&call.id, path.as_deref(), *recursive).await
            }
            (AgentToolName::Grep, AgentToolArgs::Grep { pattern, path, include }) => {
                self.grep(&call.id, pattern, path.as_deref(), include.as_deref()).await
            }
            _ => Err(AppError::Tool("tool name and arguments do not match".into())),
        }
    }
}
```

#### list_files 实现

```rust
async fn list_files(
    &self,
    call_id: &str,
    path: Option<&str>,
    recursive: bool,
) -> AppResult<AgentToolResult> {
    let root = self.fs.root();
    let target = match path {
        Some(p) => match resolve_within_root(root, p) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error(call_id, &e)),
        },
        None => root.to_path_buf(),
    };

    let mut entries = Vec::new();
    let walker = WalkDir::new(&target)
        .follow_links(false)          // 不跟随 symlink
        .max_depth(if recursive { 10 } else { 1 });

    for entry in walker.into_iter().filter_entry(|e| !is_ignored(e)) {
        let entry = match entry { Ok(e) => e, Err(_) => continue };
        if entry.path() == target.as_path() { continue; }  // 跳过起点自身
        let kind = if entry.file_type().is_dir() { "dir" } else { "file" };
        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
        entries.push(format!("{kind}  {}", rel.display()));
        if entries.len() >= LIST_FILES_MAX { break; }
    }

    let total = entries.len();
    let mut content = entries.join("\n");
    if total >= LIST_FILES_MAX {
        content.push_str(&format!("\n（结果已截断，只显示前 {LIST_FILES_MAX} 项）"));
    }
    if content.is_empty() { content = "（空目录）".into(); }

    Ok(AgentToolResult {
        tool_call_id: call_id.to_string(),
        content,
        is_error: false,
        file_change: None,
    })
}
```

#### grep 实现

```rust
async fn grep(
    &self,
    call_id: &str,
    pattern: &str,
    path: Option<&str>,
    include: Option<&str>,
) -> AppResult<AgentToolResult> {
    let re = match regex::Regex::new(pattern) {
        Ok(re) => re,
        Err(e) => return Ok(tool_error(call_id, &format!("正则编译失败: {e}"))),
    };

    let root = self.fs.root();
    let search_root = match path {
        Some(p) => match resolve_within_root(root, p) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error(call_id, &e)),
        },
        None => root.to_path_buf(),
    };

    let include_glob = include.map(|g| glob::Pattern::new(g).ok()).flatten();

    let mut matches = Vec::new();
    let walker = WalkDir::new(&search_root)
        .follow_links(false)
        .max_depth(10)
        .into_iter()
        .filter_entry(|e| !is_ignored(e));

    for entry in walker {
        let entry = match entry { Ok(e) => e, Err(_) => continue };
        if !entry.file_type().is_file() { continue; }

        // 大文件跳过
        if entry.metadata().map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(true) { continue; }

        // include glob 过滤
        let fname = entry.file_name().to_string_lossy();
        if let Some(ref g) = include_glob {
            if !g.matches(&*fname) { continue; }
        }

        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
        let content = match std::fs::read_to_string(entry.path()) { Ok(c) => c, Err(_) => continue };
        for (lineno, line) in content.lines().enumerate() {
            if re.is_match(line) {
                matches.push(format!("{}:{}: {}", rel.display(), lineno + 1, line));
                if matches.len() >= GREP_MAX { break; }
            }
        }
        if matches.len() >= GREP_MAX { break; }
    }

    let mut output = matches.join("\n");
    if matches.len() >= GREP_MAX {
        output.push_str(&format!("\n（结果已截断，只显示前 {GREP_MAX} 条匹配。请缩小搜索范围或用更精确的模式）"));
    }
    if output.is_empty() { output = "（无匹配）".into(); }

    Ok(AgentToolResult {
        tool_call_id: call_id.to_string(),
        content: output,
        is_error: false,
        file_change: None,
    })
}
```

#### 常量与辅助函数

```rust
const GREP_MAX: usize = 100;
const LIST_FILES_MAX: usize = 200;
const MAX_FILE_BYTES: u64 = 1_000_000;  // 1MB，与 WorkspaceFs::read_text 一致

const IGNORED_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build",
    ".next", ".svelte-kit", "__pycache__",
];

/// 检查 walkdir entry 是否在忽略目录里（只对目录生效，文件不过滤）。
fn is_ignored(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir()
        && entry.file_name().to_str().map(|n| IGNORED_DIRS.contains(&n)).unwrap_or(false)
}

/// 词法边界检查：把相对路径拼到 root 后规范化，确认仍在 root 内。
/// 复用 workspace.rs 的词法归并思路（不依赖 canonicalize，路径不存在也能检查）。
fn resolve_within_root(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let joined = root.join(relative);
    let normalized = lexical_normalize(&joined);
    if normalized.starts_with(root) {
        Ok(normalized)
    } else {
        Err(format!("路径越界: {relative}"))
    }
}
```

> **实现注意**:
> 1. `workspace.rs` 已有 `lexical_normalize`(私有函数,处理 `..` 归并)。把它改成 `pub(crate)`,让 tools.rs 复用(DRY),不重新实现。
> 2. `.DS_Store` 不进黑名单。它是文件不是目录,`is_ignored` 只过滤目录;让 `.DS_Store` 在 list_files 结果里出现无害(顶多多一项),强过滤反而要改 `is_ignored` 同时处理文件名,复杂度不值。

### Provider 翻译扩展(provider.rs)

`GlmProvider::tool_call_to_glm` 加两个分支:

```rust
fn tool_call_to_glm(call: &AgentToolCall) -> GlmToolCall {
    let (name, arguments) = match &call.arguments {
        AgentToolArgs::Read { path } => ("read_file", serde_json::json!({ "path": path })),
        AgentToolArgs::Write { path, content } => (
            "write_file",
            serde_json::json!({ "path": path, "content": content }),
        ),
        AgentToolArgs::ListFiles { path, recursive } => (
            "list_files",
            serde_json::json!({ "path": path, "recursive": recursive }),
        ),
        AgentToolArgs::Grep { pattern, path, include } => (
            "grep",
            serde_json::json!({ "pattern": pattern, "path": path, "include": include }),
        ),
    };
    // ...
}
```

`parse_tool_call` 加两个分支:

```rust
fn parse_tool_call(gtc: GlmToolCall) -> AppResult<AgentToolCall> {
    let name = match gtc.function.name.as_str() {
        "read_file" => AgentToolName::ReadFile,
        "write_file" => AgentToolName::WriteFile,
        "list_files" => AgentToolName::ListFiles,     // 新增
        "grep" => AgentToolName::Grep,                 // 新增
        other => return Err(AppError::Provider(format!("unknown tool: {other}"))),
    };
    let arguments = match name {
        AgentToolName::ListFiles => {
            let path = args.get("path").and_then(|v| v.as_str()).map(String::from);
            let recursive = args.get("recursive").and_then(|v| v.as_bool()).unwrap_or(false);
            AgentToolArgs::ListFiles { path, recursive }
        }
        AgentToolName::Grep => {
            let pattern = args.get("pattern").and_then(|v| v.as_str())
                .ok_or_else(|| AppError::Provider("grep missing pattern".into()))?.to_string();
            let path = args.get("path").and_then(|v| v.as_str()).map(String::from);
            let include = args.get("include").and_then(|v| v.as_str()).map(String::from);
            AgentToolArgs::Grep { pattern, path, include }
        }
        // ... 既有 ReadFile/WriteFile 不变
    };
    // ...
}
```

### tool_schemas 扩展(agent.rs)

`tool_schemas()` 加两个 schema:

```rust
fn tool_schemas() -> Vec<AgentToolSchema> {
    vec![
        // ... 既有 read_file / write_file ...
        AgentToolSchema {
            name: "list_files",
            description: "列出工作区内指定目录的文件和子目录。默认只列直接子项（不递归）。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "相对工作区根的目录路径，默认为工作区根" },
                    "recursive": { "type": "boolean", "description": "是否递归列出子目录，默认 false" }
                }
            }),
        },
        AgentToolSchema {
            name: "grep",
            description: "在工作区内搜索匹配正则表达式的文件内容。返回 path:line:content 格式的结果。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "正则表达式" },
                    "path": { "type": "string", "description": "限定搜索的目录或文件，默认整个工作区" },
                    "include": { "type": "string", "description": "文件名 glob 过滤，如 *.ts" }
                },
                "required": ["pattern"]
            }),
        },
    ]
}
```

### SYSTEM_PROMPT 更新(agent.rs)

现有 system prompt 只提了 `read_file/write_file`,GLM 不知道自己能搜索,也不会被引导"先搜再改"。更新 `SYSTEM_PROMPT` 常量:

```rust
const SYSTEM_PROMPT: &str = "你是桌面工作区 Agent。只能操作工作区内文件。

可用工具：
- list_files：列出目录内容，了解工作区结构。不确定文件在哪时，先用它探索。
- grep：按正则搜索文件内容。找某个函数/变量/字符串用在哪时用它。
- read_file：读取指定文件内容。
- write_file：写入文件（整文件覆盖）。

工作方式：
1. 不确定路径时，先 list_files 或 grep 探索，不要瞎猜路径。
2. 改文件前，先用 read_file 看当前内容。
3. 不要在回复里直接给文件内容，通过工具操作。
4. 完成任务后给出简短总结。";
```

**关键变化**:

1. **列出所有工具**:让 GLM 知道自己有 `list_files`/`grep`/`read_file`/`write_file` 四个能力。
2. **明确探索优先**:引导 GLM "不确定路径时先搜",避免瞎猜路径或直接拒绝。
3. **"改前先读"工作流**:引导 GLM 形成 `grep/list → read → write` 的自然顺序,减少 write_file 写出离谱内容。

仍然硬编码在 `agent.rs`(不可配),仅更新常量内容。

新增:

```toml
regex = "1"
glob = "0.3"        # glob::Pattern，用于 include 过滤
```

`walkdir` 已在 Cargo.toml(foundation 加的),复用。

### 前端改动

**无**。前端 `Conversation.svelte` 的 `{#each events as event}` 渲染所有工具事件,list_files/grep 的结果通过 `tool_result` 事件原样显示在中栏。`AgentEvent.kind` 复用 `tool_call`/`tool_result`,不新增 kind。

唯一可能要调整:`Conversation.svelte` 的 `tool_call_event` 里根据 `AgentToolArgs` 生成标题的逻辑(agent.rs),要加 list_files/grep 的分支:

```rust
fn tool_call_event(call: &AgentToolCall) -> AgentEvent {
    let (label, detail) = match &call.arguments {
        AgentToolArgs::Read { path } => ("read_file", path.clone()),
        AgentToolArgs::Write { path, .. } => ("write_file", path.clone()),
        AgentToolArgs::ListFiles { path, recursive } => {
            ("list_files", format!("{} (recursive={})", path.as_deref().unwrap_or("."), recursive))
        }
        AgentToolArgs::Grep { pattern, path, .. } => {
            ("grep", format!("/{pattern}/ in {}", path.as_deref().unwrap_or(".")))
        }
    };
    // ...
}
```

## 测试策略

### L1 工具层单测(tools.rs,真临时目录)

**list_files(6 个):**
1. 列空目录 → 返回"（空目录）"
2. 列有文件的目录 → 返回正确的 `file`/`dir` 列表
3. `recursive: true` → 递归列出子目录文件
4. 忽略 node_modules:建假 node_modules 目录,确认不被列出
5. 数量上限:建 250 个文件,确认只返回 200 个 + 截断提示
6. 越界路径(`../outside`)→ 返回 is_error

**grep(8 个):**
7. 搜到匹配 → 返回 `path:line:content` 格式
8. 搜不到匹配 → 返回"（无匹配）"(不是错误)
9. 正则匹配:`\binvoke\b` 只匹配单词边界
10. 忽略目录内的匹配不被返回(node_modules 里放匹配文件)
11. 跳过大文件:建 >1MB 文件含匹配,确认不被搜
12. 结果上限:建 150 处匹配,确认只返回 100 条 + 截断提示
13. `include` glob 过滤:`include: "*.ts"` 只搜 .ts 文件
14. 越界路径 → 返回 is_error

**边界(2 个,unix only):**
15. symlink 不被跟随(`#[cfg(unix)]`)
16. 深度上限:建 11 层深目录,确认第 11 层不被列(grep/list_files 各一条)

### 翻译函数单测(provider.rs,2 个)

17. `parse_tool_call("list_files", {path: "src"})` → `AgentToolArgs::ListFiles { path: Some("src"), recursive: false }`
18. `parse_tool_call("grep", {pattern: "invoke", include: "*.ts"})` → `AgentToolArgs::Grep { pattern: "invoke", path: None, include: Some("*.ts") }`

### 不加 L2

循环逻辑不变,现有 4 个 L2 测试已覆盖。新工具的分发靠 L1 覆盖。

## 成功标准(验收清单)

做完后能完成以下场景:

1. **浏览结构**:输入"看看这个项目有哪些文件",Agent 调 `list_files` 列出顶层结构,正确识别 src/、src-tauri/、package.json 等。
2. **找内容**:输入"找到所有用了 invoke 的地方",Agent 调 `grep` 搜 `invoke`,返回 `src/lib/api.ts:12:...` 这类结果。
3. **组合任务**:输入"找到所有 .ts 文件里的 export,然后告诉我有哪些",Agent 调 `grep({pattern: "export", include: "*.ts"})` 完成搜索并总结。
4. **安全**:让 Agent 搜 `../etc`,工具返回 is_error;node_modules 里的匹配不被返回。
5. **测试**:`cargo test` 全绿(含 18 个新测试)、`pnpm check`/`pnpm test`/`pnpm build` 全绿。

## 后续计划(明确不做)

- **命令执行器**:让 Agent 能跑 `cargo check`/`rg`/`git commit`,含风险确认 UI。独立计划。
- **rg 后端**:检测到机器装了 ripgrep 时,把纯 Rust 搜索换成 rg,提升大工作区性能。接口已预留。
- **完整 .gitignore 解析**:支持嵌套 .gitignore、negation 语法。
- **edit_file**(search-replace):解决 write_file 改大文件烧 token 的问题。优先级高于命令执行器。
- **匹配行上下文**:grep 返回匹配行前后 N 行,减少 Agent 二次 read_file 的次数。
