# 多 Provider 支持设计规格

**日期**:2026-06-15
**关联**:承接之前所有工具计划。GLM 流量不够，需要接 MiniMax 作为第二个模型 Provider。

## 目标

让 Agent 支持 GLM 和 MiniMax 两个模型 Provider，用户通过 config.toml 的 `active` 字段切换。

## 核心洞察

GLM 和 MiniMax 的 API **完全同构**——都走 OpenAI Chat Completions 兼容格式（`role`/`content`/`tool_calls`/`tool_call_id`、`/chat/completions` 端点、`Bearer` 鉴权、多轮 function call 完整回传）。唯一区别是 base_url 和 model name。

因此不需要写第二个 Provider 类——把现有的 `GlmProvider` 重命名为通用的 `OpenAICompatibleProvider`，通过 config 的 base_url + model 适配任何 OpenAI 兼容 API。

## 非目标

- **不做设置页 UI 切换**。切换通过手编辑 config.toml 的 `active` 字段，设置页只读展示当前 Provider。
- **不做 `save_config` 命令**。config 仍只读（读取层扩展，不新增写入）。
- **不接非 OpenAI 兼容的 Provider**（如 Claude 原生 Messages API、Gemini）。如果以后要接，那时再写独立的 Provider 类实现 `AgentProvider` trait。
- **不做 Provider 健康检查/自动 fallback**。用户手动选 active Provider，失败就失败。

## 核心决策

| # | 决策 | 选择 |
|---|------|------|
| 1 | 实现方式 | 重命名 `GlmProvider` → `OpenAICompatibleProvider`，零新增 Provider 类 |
| 2 | DTO 命名 | `GlmMessage` → `OpenAIMessage`，`GlmToolCall` → `OpenAIToolCall` 等，全 `OpenAI*` 前缀 |
| 3 | 配置格式 | config.toml 支持多 Provider 段 + `active` 字段，向后兼容旧平铺格式 |
| 4 | 切换方式 | 手编辑 `active = "glm"` 或 `active = "minimax"` |
| 5 | ConfigStatus | 加 `provider` 字段，设置页显示当前 Provider |
| 6 | 默认值 | 旧格式默认 GLM；新格式必须有 `active`，缺失则报错 |

## 架构

### 重命名映射

| 旧名 | 新名 |
|------|------|
| `GlmProvider` | `OpenAICompatibleProvider` |
| `GlmMessage` | `OpenAIMessage` |
| `GlmToolCall` | `OpenAIToolCall` |
| `GlmFunction` | `OpenAIFunction` |
| `GlmToolDef` | `OpenAIToolDef` |
| `GlmToolFunctionDef` | `OpenAIToolFunctionDef` |
| `GlmRequest` | `OpenAIRequest` |
| `GlmResponse` | `OpenAIResponse` |
| `GlmChoice` | `OpenAIChoice` |

**翻译函数也重命名**：`turn_to_glm_message` → `turn_to_openai_message`、`tool_call_to_glm` → `tool_call_to_openai`、`tool_schema_to_glm` → `tool_schema_to_openai`、`translate_response`（保持）、`parse_tool_call`（保持）。

**测试里的引用也跟着改**（`GlmResponse` → `OpenAIResponse` 等）。

### Config schema 扩展(config.rs)

#### 新格式（多 Provider 段）

```toml
active = "minimax"   # "glm" 或 "minimax"

[glm]
api_key = "glm-key"
model = "glm-4.6"                    # 可选，默认 glm-4.6
base_url = "https://open.bigmodel.cn/api/paas/v4"  # 可选，有默认值

[minimax]
api_key = "minimax-key"
model = "MiniMax-M3"                 # 可选，默认 MiniMax-M3
base_url = "https://api.minimax.io/v1"  # 可选，有默认值
```

#### 旧格式（平铺，向后兼容）

```toml
api_key = "xxx"
model = "glm-4.6"
base_url = "https://open.bigmodel.cn/api/paas/v4"
```

#### 解析逻辑

```rust
impl AgentConfig {
    pub fn load() -> AppResult<(Self, String)> {
        let content = fs::read_to_string(config_path()?)...;
        
        // 尝试解析新格式
        #[derive(Deserialize)]
        struct MultiProviderConfig {
            active: Option<String>,
            glm: Option<ProviderEntry>,
            minimax: Option<ProviderEntry>,
        }
        #[derive(Deserialize)]
        struct ProviderEntry {
            api_key: String,
            #[serde(default)] model: Option<String>,
            #[serde(default)] base_url: Option<String>,
        }

        // 也尝试旧格式
        #[derive(Deserialize)]
        struct LegacyConfig {
            #[serde(default)] api_key: Option<String>,
            #[serde(default)] model: Option<String>,
            #[serde(default)] base_url: Option<String>,
        }

        // 先试多 Provider 格式
        if let Ok(multi) = toml::from_str::<MultiProviderConfig>(&content) {
            if multi.active.is_some() {
                let active = multi.active.unwrap();
                let entry = match active.as_str() {
                    "glm" => multi.glm.ok_or(...)?,
                    "minimax" => multi.minimax.ok_or(...)?,
                    other => return Err(...unknown provider...),
                };
                let defaults = provider_defaults(&active);
                return Ok((AgentConfig {
                    api_key: entry.api_key,
                    model: entry.model.unwrap_or_else(|| defaults.model),
                    base_url: entry.base_url.unwrap_or_else(|| defaults.base_url),
                }, active));
            }
        }

        // 回退到旧格式（平铺）
        let legacy: LegacyConfig = toml::from_str(&content)?;
        if legacy.api_key.as_deref().map(|k| !k.trim().is_empty()).unwrap_or(false) {
            return Ok((AgentConfig {
                api_key: legacy.api_key.unwrap(),
                model: legacy.model.unwrap_or_else(|| "glm-4.6".to_string()),
                base_url: legacy.base_url.unwrap_or_else(|| "https://open.bigmodel.cn/api/paas/v4".to_string()),
            }, "glm".to_string()));
        }

        Err(AppError::ConfigNotConfigured)
    }
}

fn provider_defaults(name: &str) -> Defaults {
    match name {
        "glm" => Defaults { model: "glm-4.6".into(), base_url: "https://open.bigmodel.cn/api/paas/v4".into() },
        "minimax" => Defaults { model: "MiniMax-M3".into(), base_url: "https://api.minimax.io/v1".into() },
        _ => Defaults { model: "unknown".into(), base_url: "".into() },
    }
}
```

**注意**：`AgentConfig::load()` 的返回类型从 `AppResult<Self>` 变成 `AppResult<(Self, String)>`（多了 provider name）。调用方（`lib.rs` 的 `run_agent_task` 命令）相应调整。

### ConfigStatus 扩展(domain.rs)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStatus {
    pub configured: bool,
    pub provider: String,   // 新增："glm" / "minimax"
    pub model: String,
}
```

`AgentConfig::status()` 返回 provider name：

```rust
pub fn status() -> ConfigStatus {
    match Self::load() {
        Ok((c, provider)) => ConfigStatus { configured: true, provider, model: c.model },
        Err(_) => ConfigStatus { configured: false, provider: "(未配置)".into(), model: "(未配置)".into() },
    }
}
```

### 前端改动

**`types.ts`**: `ConfigStatus` 加 `provider` 字段：
```typescript
export interface ConfigStatus {
  configured: boolean;
  provider: string;   // 新增
  model: string;
}
```

**`SettingsPanel.svelte`**: 显示当前 Provider：
```svelte
<p>Provider: {status.provider}</p>
<p>Model: {status.model}</p>
```

### lib.rs 调整

`run_agent_task` 命令里，`AgentConfig::load()` 返回值解构：

```rust
let (config, _provider) = AgentConfig::load()?;
let provider = OpenAICompatibleProvider::new(config);
// ...
```

## 受影响的测试

### config 测试（config.rs 的测试要改返回类型）

现有 4 个 config 测试断言 `AgentConfig::load()` 的返回值。返回类型从 `Self` 变成 `(Self, String)`，测试里解构改成 `let (cfg, provider) = AgentConfig::load().unwrap();`，并加 `assert_eq!(provider, "glm")`（旧格式默认 glm）。

新增测试：
- 多 Provider 格式，active=glm → 返回 GLM 的 config + provider="glm"
- 多 Provider 格式，active=minimax → 返回 MiniMax 的 config + provider="minimax"
- 多 Provider 格式，active=unknown → 报错
- 多 Provider 格式，active 指定但对应段缺失 → 报错

### 翻译测试（provider.rs 测试里 DTO 改名）

现有 4 个 GLM 翻译测试里的 `GlmResponse`/`GlmChoice`/`GlmMessage`/`GlmToolCall`/`GlmFunction` 引用，全改成 `OpenAIResponse`/`OpenAIChoice`/`OpenAIMessage`/`OpenAIToolCall`/`OpenAIFunction`。`GlmProvider::translate_response` → `OpenAICompatibleProvider::translate_response`，`GlmProvider::turn_to_openai_message` → `OpenAICompatibleProvider::turn_to_openai_message`。

测试逻辑不变，只是类型名变。

## 成功标准

1. **旧 config 向后兼容**：现有的平铺 config.toml 正常工作，Agent 用 GLM。
2. **新 config 多 Provider**：配好 `[glm]` + `[minimax]` 两个段 + `active = "minimax"`，Agent 切到 MiniMax。
3. **切换只改一行**：`active = "glm"` ↔ `active = "minimax"`。
4. **设置页显示 Provider**：显示「Provider: minimax | model: MiniMax-M3」。
5. **MiniMax 真能用**：输入任务，MiniMax 真调 API，Agent 真改文件（和 GLM 一样的体验）。
6. **测试**：cargo test 全绿、pnpm check/test/build 全绿。

## 后续计划

- **设置页 UI 切换**：下拉框选 Provider + save_config 命令写回 config.toml。
- **非 OpenAI 兼容 Provider**：Claude 原生 Messages API、Gemini 等，需要独立 Provider 类。
- **Provider 健康检查/自动 fallback**：active Provider 挂了自动切到备用。
