# 网络工具设计规格（web_search + web_fetch）

**日期**: 2026-06-17
**关联**: 给 sophoni 补全工具箱的第一梯队能力 —— 网络信息获取。承接现有 9 个工具（read_file/write_file/edit_file/list_files/grep/run_command + 3 个验收工具），新增 `web_search` + `web_fetch`，让 agent 从「只能看本地」变成「能查外部信息」。

## 目标

解决 agent 遇到未知报错、陌生 API、版本兼容性问题时**只能靠读本地代码猜**的问题：

- **`web_search`**: 搜索网络，返回标题+摘要+URL 列表。
- **`web_fetch`**: 抓取指定 URL 的页面内容，转为文本。

两者配套使用：search 找到线索 → fetch 读详情。

## 非目标（明确不做）

- **不做通用 HTTP 工具**（任意 method/body）。只做搜索 + 只读抓取。
- **不做 `multi_edit_file` / `delete_file`** 等文件工具。本次只做网络两件套。
- **不调整 git 安全策略**。git 已通过 `run_command` + 风险白名单覆盖。
- **不做缓存层**。每次搜索/抓取都实时请求，缓存留后续。
- **不做前端新组件**。复用现有 `ToolCallCard`（只读工具统一卡片）。

## 核心决策（对话中确认）

| # | 决策 | 选择 |
|---|------|------|
| 1 | 本次范围 | 只做 web_search + web_fetch，网络是唯一的能力质变 |
| 2 | 搜索后端 | 同时支持 Tavily + Google CSE，**fallback 模式**（优先 Tavily，没配则 Google） |
| 3 | 配置结构 | 独立 `[search]` 段，与 LLM provider 解耦 |
| 4 | web_fetch SSRF 防护 | 加内网 IP 过滤（拒绝 127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, ::1, fc00::/7, localhost） |
| 5 | web_search schema | 总是暴露给模型（无配置时返回友好错误，而非隐藏工具） |
| 6 | ChatOnly 模式 | 网络工具不受 ChatOnly 拦截（纯对话模式也能查网络） |
| 7 | 前端展示 | 复用 ToolCallCard，零新增组件 |

## 架构

### 与现有代码的关系

```
src-tauri/src/core/
├── web.rs          [新增] 搜索后端 + web_fetch 实现 + SSRF 防护
├── domain.rs       [改]   AgentToolName/AgentToolArgs 加 2 个变体；AgentConfig 加 search_config
├── config.rs       [改]   解析 [search] 段
├── tools.rs        [改]   ToolDispatcher 加 search_config + dispatch 2 个分支（不受 ChatOnly 拦截）
├── agent.rs        [改]   tool_schemas 加 2 个 schema；SYSTEM_PROMPT 加网络工具说明
├── provider.rs     [改]   tool_call_to_openai / parse_tool_call 加 2 个工具翻译
└── lib.rs          [改]   构造 ToolDispatcher 时传入 search_config
src-tauri/Cargo.toml [改]  加 html2text 依赖
src/lib/components/SettingsPanel.svelte [改] 加网络搜索配置区
```

模块依赖方向（无环）：

```
agent → tools → web → (reqwest, html2text)
             → workspace → domain
provider → domain
config → domain
```

### 网络模块（web.rs）

```rust
use std::net::IpAddr;

/// 从配置文件 [search] 段反序列化
#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    #[serde(default)]
    pub tavily_key: Option<String>,
    #[serde(default)]
    pub google_key: Option<String>,
    #[serde(default)]
    pub google_cx: Option<String>,
}

impl SearchConfig {
    /// 是否有任意一个可用的搜索后端
    pub fn has_backend(&self) -> bool {
        self.tavily_key.is_some() || (self.google_key.is_some() && self.google_cx.is_some())
    }
}

pub struct SearchResult {
    pub title: String,
    pub snippet: String,
    pub url: String,
}

/// 搜索后端抽象，Tavily 和 Google CSE 各一实现
#[async_trait]
pub trait WebSearchBackend: Send + Sync {
    async fn search(&self, query: &str, max_results: usize) -> AppResult<Vec<SearchResult>>;
}

/// 按配置选择后端：优先 Tavily，fallback Google CSE
pub fn select_backend(config: &SearchConfig) -> Option<Box<dyn WebSearchBackend>> {
    if let Some(key) = &config.tavily_key {
        return Some(Box::new(TavilyBackend::new(key.clone())));
    }
    if let (Some(key), Some(cx)) = (&config.google_key, &config.google_cx) {
        return Some(Box::new(GoogleCseBackend::new(key.clone(), cx.clone())));
    }
    None
}

/// 格式化搜索结果为模型友好的文本
pub fn format_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "（无搜索结果）".to_string();
    }
    results
        .iter()
        .enumerate()
        .map(|(i, r)| format!("[{}] {}\n    {}\n    {}", i + 1, r.title, r.url, r.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// SSRF 防护：拒绝内网地址
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()           // 127.0.0.0/8
                || v4.is_private()      // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
                || v4.is_link_local()   // 169.254.0.0/16
                || v4.is_unspecified()  // 0.0.0.0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()           // ::1
                || v6.is_unspecified()  // ::
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 (unique local)
        }
    }
}

/// web_fetch：抓取 URL 并转为文本
pub async fn web_fetch(client: &reqwest::Client, url: &str, max_chars: usize) -> AppResult<String> {
    // 1. 校验 scheme（只允许 http/https）
    // 2. 解析域名 → 查 DNS → 校验所有 IP 非内网
    // 3. HTTP GET，限制 Content-Type 为 text/*
    // 4. HTML → markdown（html2text crate）
    // 5. 截断到 max_chars
    // 详情见下文实现要点
}
```

### web_search 工具规格

```json
{
  "name": "web_search",
  "description": "搜索网络获取外部信息。遇到未知报错、陌生 API、版本兼容性问题时使用。返回标题、摘要和 URL 列表。",
  "parameters": {
    "type": "object",
    "properties": {
      "query": { "type": "string", "description": "搜索关键词" },
      "max_results": { "type": "integer", "minimum": 1, "maximum": 10, "description": "返回条数，默认 5" }
    },
    "required": ["query"]
  }
}
```

**返回格式（给模型的 content）：**
```
[1] Rust tokio timeout error - Stack Overflow
    https://stackoverflow.com/questions/12345/...
    错误提示 tokio::time::timeout 需要 Future 实现 Unpin...

[2] Tokio timeout documentation
    https://docs.rs/tokio/...
    tokio::time::timeout 在指定时间后返回 Err(Elapsed)...
```

**Tavily 实现：**
- POST `https://api.tavily.com/search`
- body: `{ "query": "...", "max_results": N, "search_depth": "basic" }`
- header: `Authorization: Bearer <key>`
- 响应 `results[].{title, content, url}` → SearchResult

**Google CSE 实现：**
- GET `https://www.googleapis.com/customsearch/v1?q=<query>&key=<key>&cx=<cx>&num=<N>`
- 响应 `items[].{title, link, snippet}` → SearchResult

**错误处理：**
- 未配置后端 → `is_error=true`，content: "未配置搜索 API，请在设置里配置 Tavily 或 Google CSE key"
- key 无效 / 配额用完 → `is_error=true`，透传 API 错误信息
- 0 结果 → content: "（无搜索结果）"，`is_error=false`

### web_fetch 工具规格

```json
{
  "name": "web_fetch",
  "description": "抓取指定 URL 的网页内容并转为文本。用于读取 web_search 找到的页面详情（文档、Stack Overflow 答案、GitHub issue 等）。返回页面的正文文本（HTML 已转为 markdown）。",
  "parameters": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "要抓取的完整 URL（http/https）" },
      "max_chars": { "type": "integer", "minimum": 500, "maximum": 50000, "description": "返回的最大字符数，默认 8000" }
    },
    "required": ["url"]
  }
}
```

**实现流程：**
```
1. 校验 URL scheme（只允许 http/https），非法 → error
2. SSRF 防护：
   - 解析域名
   - tokio::net::lookup_host 查 DNS
   - 遍历所有返回的 IP，任何一个命中 is_private_ip → 拒绝
   - 解析失败（无 DNS 记录）→ 拒绝
3. HTTP GET（复用 lib.rs 现有 reqwest::Client，30s 超时）
   - 校验 Content-Type 以 text/ 开头（拒绝二进制/图片/视频）
   - 限制响应体：读取最多 1MB，超过截断
4. HTML → markdown 转换（html2text crate）
5. 截断到 max_chars，追加 "\n（内容已截断，显示前 N 字符）"
```

**错误处理：**
- 内网地址 → `is_error=true`，content: "拒绝抓取内网地址"
- 非 text Content-Type → `is_error=true`，content: "不支持的页面类型: <content-type>"
- HTTP 错误（4xx/5xx）→ `is_error=true`，content: "抓取失败: HTTP <status>"
- 网络超时 → `is_error=true`，content: "抓取超时(30s)"

### domain.rs 扩展

```rust
pub enum AgentToolName {
    ReadFile,
    WriteFile,
    ListFiles,
    Grep,
    EditFile,
    ReadAcceptanceReport,
    ReadRuntimeLog,
    ListAcceptanceRuns,
    RunCommand,
    WebSearch,   // 新增
    WebFetch,    // 新增
}

pub enum AgentToolArgs {
    // ... 既有 ...
    WebSearch { query: String, max_results: usize },
    WebFetch { url: String, max_chars: usize },
}
```

`AgentConfig` 加字段：
```rust
pub struct AgentConfig {
    // ... 既有 ...
    pub search_config: Option<SearchConfig>,  // 新增，None = 未配置网络搜索
}
```

### config.rs 扩展

解析 TOML 时读取 `[search]` 段：
```toml
[search]
tavily_key = "tvly-xxx"
google_key = "xxx"
google_cx = "xxx:yyy"
```

- 段不存在或所有字段为空 → `search_config = None`
- 与多 provider 格式兼容（`[search]` 与 `[glm]`/`[minimax]` 平级）

### tools.rs 扩展

`ToolDispatcher` 加 `search_config` 字段：
```rust
pub struct ToolDispatcher {
    fs: WorkspaceFs,
    risk_level: RiskLevel,
    confirm_handler: Option<Arc<dyn ConfirmHandler>>,
    workspace_mode: WorkspaceMode,
    search_config: Option<SearchConfig>,  // 新增
    http_client: reqwest::Client,         // 新增，复用于 web_fetch
}
```

**dispatch 关键改动**：web_search / web_fetch **在 ChatOnly 检查之前放行**：

```rust
pub async fn dispatch(&self, call: &AgentToolCall) -> AppResult<AgentToolResult> {
    // 网络工具不受 ChatOnly 拦截（纯对话模式也能查网络）
    match (&call.name, &call.arguments) {
        (AgentToolName::WebSearch, AgentToolArgs::WebSearch { query, max_results }) => {
            return self.web_search(&call.id, query, *max_results).await;
        }
        (AgentToolName::WebFetch, AgentToolArgs::WebFetch { url, max_chars }) => {
            return self.web_fetch(&call.id, url, *max_chars).await;
        }
        _ => {}
    }
    // ChatOnly 拦截（仅对文件/命令工具）
    if self.workspace_mode == WorkspaceMode::ChatOnly { ... }
    // 既有 match 分支...
}
```

`web_search` 实现：
- `search_config` 为 None → 返回友好错误
- `select_backend` 选后端 → `backend.search()` → `format_results()` → content

`web_fetch` 实现：
- 调 `web::web_fetch(&self.http_client, url, max_chars)`
- 成功 → content 为页面文本；失败 → `tool_error`

### provider.rs 扩展

`tool_call_to_openai` 加 2 分支：
```rust
AgentToolArgs::WebSearch { query, max_results } => (
    "web_search",
    serde_json::json!({ "query": query, "max_results": max_results }),
),
AgentToolArgs::WebFetch { url, max_chars } => (
    "web_fetch",
    serde_json::json!({ "url": url, "max_chars": max_chars }),
),
```

`parse_tool_call`（provider.rs 翻译响应）加 2 分支：解析 `web_search`/`web_fetch` 的 name + arguments。

### agent.rs 扩展

`tool_schemas` 加 2 个 schema（总是返回，不论是否配置 search）。

`tool_call_event` 加 2 分支（前端展示用 title）：
```rust
AgentToolArgs::WebSearch { query, .. } => ("web_search", query.clone(), format!("query: {query}")),
AgentToolArgs::WebFetch { url, .. } => ("web_fetch", url.clone(), format!("url: {url}")),
```

`SYSTEM_PROMPT` 工具列表加：
```
- web_search：搜索网络。遇到未知报错、陌生 API、不确定的用法时，先搜索而不是猜。
- web_fetch：读取网页内容。web_search 找到线索后，用它读取详情。
```

### 前端改动

**SettingsPanel.svelte**：加"网络搜索"配置区，3 个输入框（Tavily Key / Google Key / Google CX），保存到 `[search]` 段。需要新增 IPC 命令 `save_search_config` / `get_search_config`。

**Conversation.svelte**：无改动。web_search/web_fetch 的 `tool_call` 事件 title 形如 `web_search: rust tokio` / `web_fetch: https://...`，被现有 `processTurns` 兜底分支捕获，用 `ToolCallCard` 渲染。

## 测试策略

### web.rs 单测

**SSRF 防护（纯函数，无网络）：**
1. `is_private_ip(127.0.0.1)` → true
2. `is_private_ip(10.0.0.1)` → true
3. `is_private_ip(192.168.1.1)` → true
4. `is_private_ip(8.8.8.8)` → false
5. `is_private_ip(::1)` → true
6. `format_results` 空结果 → "（无搜索结果）"
7. `format_results` 多条 → 序号+标题+URL+摘要格式正确
8. `select_backend` 有 tavily_key → 返回 TavilyBackend
9. `select_backend` 只有 google → 返回 GoogleCseBackend
10. `select_backend` 都没有 → None

**web_fetch（需要 mock HTTP，用 httpmock crate）：**
11. 正常 HTML 页面 → 返回转文本后的内容
12. 超过 max_chars → 截断 + 提示
13. 非 text Content-Type（如 image/png）→ error
14. 内网 URL（mock DNS 解析到 127.0.0.1）→ 拒绝（注：mock 难度大，可只测 is_private_ip 逻辑，端到端留手动验证）
15. 非 http/https scheme → error

### tools.rs 单测

16. web_search 未配置 search_config → 友好错误（is_error）
17. web_search ChatOnly 模式不拦截（对比文件工具被拦截）

### provider.rs 单测

18. `parse_tool_call("web_search", {query: "rust"})` → 正确变体
19. `parse_tool_call("web_fetch", {url: "https://..."})` → 正确变体

## 成功标准

1. **查报错**：遇到陌生编译错误时，agent 主动 `web_search` 搜索错误信息，`web_fetch` 读解决方案。
2. **查 API**：不确定某 API 用法时，`web_search` 找官方文档，`web_fetch` 读文档页。
3. **未配置优雅降级**：没配 search key 时，`web_search` 返回友好错误，agent 不会卡死。
4. **SSRF 防护**：`web_fetch` 拒绝内网地址。
5. **ChatOnly 可用**：纯对话模式下 `web_search`/`web_fetch` 正常工作（文件工具被拦但网络工具放行）。
6. **前端展示**：搜索结果和抓取内容在 `ToolCallCard` 里可展开查看。
7. **测试全绿**：`cargo test` + `pnpm test` + `pnpm check` 通过，`pnpm accept` ok=true。

## 依赖

```toml
# Cargo.toml 新增
html2text = "0.12"   # HTML → 纯文本/markdown 转换
httpmock = "0.8"      # [dev-dependencies] mock HTTP 用于测试
```

`reqwest` 已在 Cargo.toml（provider 用的），复用。

## 后续计划（明确不做）

- **搜索结果缓存**：相同 query 短时间内复用结果。
- **Brave Search / Bing** 后端：等有需求再加。
- **web_fetch 的 JS 渲染**：当前不执行 JS，纯服务端 HTML；动态站点（SPA）抓不到内容时留后续。
- **`multi_edit_file` / `delete_file`**：第二梯队文件工具，等网络两件套稳定后做。
