# 跨任务上下文记忆

## 背景与动机

当前每个新任务都是全新对话——Agent 看不到之前任务做过什么。用户在同一工作区连续工作时，每次都要重新告诉 Agent 上下文（"我之前改了 lib.rs"、"项目用 pnpm 不是 npm"），体验割裂。

本次新增跨任务上下文记忆——新任务开始时，Agent 能看到同一工作区历史任务的聚合摘要，按任务类别分组。

## 设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 记忆内容 | 聚类摘要（按 category 分组）+ 最近任务摘要 | token 省，上下文连贯 |
| 类别划分 | LLM 自动归类 | 每个任务结束时让模型输出 category 标签 |
| 聚合时机 | 新任务开始时实时拼接 | 不需要额外 LLM 调用，按 category 分组拼接 summary |
| 传输方式 | 作为 conversation 前缀（assistant turn） | 模型看到的是"你之前做过什么"，不是完整历史对话 |
| 跨工作区 | 不做 | 记忆绑定到当前工作区 |

## 记忆策略示例

工作区有 10 个历史任务：
- 任务 1-3：编译修复类（category="编译修复"）
- 任务 4-8：依赖管理类（category="依赖管理"）
- 任务 9-10：文档更新类（category="文档更新"）
- 任务 10 是最近一个任务

新任务开始时，传给模型的记忆文本：

```
## 历史任务记忆

### 编译修复
- 修复了 lib.rs 缺分号，cargo check 通过
- 修复了 Cargo.toml 版本冲突
- 修复了 workspace.rs 的路径逃逸检查

### 依赖管理
- 添加了 tokio process 依赖
- 升级了 rusqlite 到 0.31
- 添加了 tauri-plugin-dialog
- 添加了 async-trait
- 移除了未使用的 serde_json 直接依赖

### 文档更新
- 更新了 README 能力清单
- 添加了风险等级使用说明

### 最近任务
- 添加了风险等级使用说明
```

## 数据模型

### conversations 表加 category 字段

```sql
ALTER TABLE conversations ADD COLUMN category TEXT;
```

`category` 可为 NULL（老数据无类别，或归类失败时）。默认值 NULL。

### category 生成方式

在 SYSTEM_PROMPT 的工作方式里加一条规则，要求模型在 FinalAnswer 里带上 category 标记：

```
10. 任务完成后的总结，第一行必须是分类标签，格式 [category: 标签名]。标签用 2-4 个字概括任务类型（如"编译修复"、"依赖管理"、"文档更新"、"命令执行"、"测试验证"）。
```

Agent 循环解析 FinalAnswer：第一行 `[category: xxx]` 提取为 category，剩余文本为 summary。如果没找到标记，category 为 NULL。

### storage 新增方法

| 方法 | 说明 |
|------|------|
| `list_conversation_memories(workspace_id) -> Vec<ConversationMemory>` | 读所有历史（category + summary + updated_at），按 updated_at 升序 |
| `update_conversation_category(id, category)` | 任务结束时写入 category |

`ConversationMemory` 结构：

```rust
pub struct ConversationMemory {
    pub category: Option<String>,
    pub summary: String,
    pub updated_at: DateTime<Utc>,
}
```

## 记忆构造逻辑

新任务开始时（`run_agent_task` 内部）：

```
1. 从 DB 读 workspace_id 下所有 conversation 的 (category, summary, updated_at)
2. 提取已有的 category 去重列表，写入 prompt 引导模型复用
3. 按 category 分组（NULL 归入"其他"）
4. 每组内按时间排序，拼接 summary（"- {summary}"）
5. 取最近一个任务的 summary 单独列为"最近任务"
6. 组装成记忆文本
7. 作为 conversation 前缀传给模型
```

### Category 一致性：prompt 引导复用

新任务开始时，从 DB 读出已有的 category 列表（去重），写进 system prompt 的工作方式第 10 条：

```
10. 任务完成后的总结，第一行必须是分类标签，格式 [category: 标签名]。
    已有类别：编译修复、依赖管理、文档更新。
    优先复用已有类别，只在新任务类型不属于任何已有类别时才创建新类别。
    标签用 2-4 个字概括任务类型。
```

"已有类别"列表从记忆构造逻辑第 2 步动态生成。首个任务（无历史）时该行为空，模型自由创建第一个类别。

组装函数（agent.rs 或独立模块）：

```rust
fn build_memory_context(memories: &[ConversationMemory]) -> String {
    if memories.is_empty() {
        return String::new();
    }

    let mut by_category: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for m in memories {
        let cat = m.category.as_deref().unwrap_or("其他");
        by_category.entry(cat.to_string()).or_default().push(&m.summary);
    }

    let mut text = String::from("## 历史任务记忆\n\n");
    for (category, summaries) in &by_category {
        text.push_str(&format!("### {category}\n"));
        for s in summaries {
            text.push_str(&format!("- {s}\n"));
        }
        text.push('\n');
    }

    // 最近任务
    if let Some(last) = memories.last() {
        text.push_str(&format!("### 最近任务\n- {}\n", last.summary));
    }

    text
}
```

## Agent 循环集成

### run_agent_task 改动

`run_agent_task` 接收 `memory_context: String` 参数。

在构造 conversation turns 时，如果有 memory_context，加一个 assistant turn 作为前缀：

```rust
let mut turns: Vec<ConversationTurn> = vec![];
if !memory_context.is_empty() {
    turns.push(ConversationTurn::Assistant {
        content: memory_context,
    });
}
turns.push(ConversationTurn::User { content: user_task });
```

### lib.rs 改动

`run_agent_task` 命令在创建 conversation 前，从 storage 读历史记忆：

```rust
let memories = storage.list_conversation_memories(&workspace_id);
let memory_context = build_memory_context(&memories);
```

传给 `run_agent_task_inner(..., &memory_context)`。

### FinalAnswer 解析 category

Agent 循环拿到 FinalAnswer 后：

```rust
let (category, clean_summary) = parse_category(&text);
```

```rust
fn parse_category(text: &str) -> (Option<String>, String) {
    let first_line = text.lines().next().unwrap_or("");
    if let Some(rest) = first_line.strip_prefix("[category:") {
        let category = rest.trim_end_matches(']').trim().to_string();
        let clean = text.lines().skip(1).collect::<Vec<_>>().join("\n");
        (Some(category), clean.trim().to_string())
    } else {
        (None, text.to_string())
    }
}
```

任务结束后写 DB 时：
- `events_json` 存完整 events（含原始 FinalAnswer）
- `title` 用 clean_summary
- `category` 写入新字段

## SYSTEM_PROMPT 调整

`system_prompt(level, mode, existing_categories)` 加一条工作方式（Full 模式下），动态插入已有类别：

```
10. 任务完成后的总结，第一行必须是分类标签，格式 [category: 标签名]。
    {已有类别提示，如："已有类别：编译修复、依赖管理。优先复用已有类别，只在新任务类型不属于任何已有类别时才创建新类别。"}
    标签用 2-4 个字概括任务类型。从第二行开始写总结。
```

`existing_categories` 从 DB 读取（当前工作区的历史 category 去重）。首个任务时该行为空，模型自由创建第一个类别。

ChatOnly 模式不加（纯对话不需要分类）。

## 成功标准

1. **category 生成**：任务完成后 DB 里有 category 值（如"编译修复"）。
2. **记忆拼接**：同 category 的历史 summary 聚合到一起。
3. **跨任务记忆**：第二个任务开始时，Agent 能看到第一个任务的摘要（在对话中引用历史上下文）。
4. **最近任务**：记忆文本包含最近任务的单独摘要。
5. **向后兼容**：无 category 的老数据归入"其他"。
6. **验收通过**：`pnpm accept` ok=true。

## 明确不做（YAGNI）

- **完整历史对话传给模型** — 只传聚合摘要。
- **跨工作区记忆** — 记忆绑定当前工作区。
- **记忆手动编辑/删除** — 后续。
- **token 预算自动截断** — 先不限制类别数量，实际使用观察后再加。
- **category 的 i18n** — 中文标签即可。
- **记忆的向量检索** — 不做语义搜索，纯按 category 分组。
