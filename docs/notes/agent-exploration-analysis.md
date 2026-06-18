# Agent 探索行为分析笔记

> 用于积累样本，待数据充足后再决定是否针对性优化 prompt。

## 背景

用户反馈 agent 整理工程架构时「倾向于深度优先读取」。记录观察到的现象和已做的优化，避免重复分析。

## 已实施的优化（2026-06-18）

针对「大任务跑满轮次失败」，做了四层修复：

| 改动 | 文件 | 作用 |
|------|------|------|
| system_prompt 探索策略改写 | agent.rs | 引导「list_files recursive 一次列全 → 针对性深入」，避免逐目录浅层探索 |
| category_rule 预存 bug 修复 | agent.rs | 分类规则算了但没拼进 prompt，现作为编号 10 正确拼入 |
| MAX_ROUNDS 12→20，OVERALL_TIMEOUT 120s→300s | agent.rs | 给复杂任务更多余量 |
| 轮次预算提醒 | agent.rs | 剩余 ≤5 轮时注入 User 角色系统提醒，打断探索惯性迫使收敛（哨兵只注入一次）|
| list_files 自动附关键文件摘要 | tool_spec.rs | package.json/Cargo.toml 等入口配置附前 20 行/500 字符，减少 read_file 轮次 |
| 单轮超时改无活动超时 | provider.rs + agent.rs | 30s 总时长超时 → 60s 无活动超时，长结果流式输出不再被误杀 |
| reqwest connect_timeout | provider.rs | `.timeout(30s)` 总超时误杀流式读取 → `.connect_timeout(30s)` 只限连接阶段 |

## 观察样本

### 样本 1：captain 工程（2026-06-18 12:12，会话 9de5b267）✅ 成功

- **工程**：Cocos Creator 2.4.15 老虎机游戏
- **结果**：14 轮完成，给出最终架构总结（成功，未跑满）
- **工具分布**：13 次 list_files + 24 次 read_file（read 远多于 list）
- **最终答案耗时**：轮 14 花了 **98 秒**（之前会被 30s 超时砍掉，现修复后能输出）
- **轮次预算提醒**：未触发（14 轮完成，没到剩 5 轮的阈值 round=15）

**探索时序（深度优先倾向明显）：**
```
轮1:  list_files . recursive=true       ← ✅ 广度优先，prompt 生效
轮2-3: read_file 4个配置文件            ← 立刻钻进配置细节
轮4:  list 2个子目录                    ← 一点广度
轮5:  list 6个子目录                    ← 广度
轮6:  list assets/Sweet/script (单目录) ← 又钻进单个子目录
轮7-13: read_file 逐个深入              ← 深度优先读细节
轮14: 最终答案（98秒）
```

**关键证据（thought 推理）：**
- thought 1: "让我先用 list_files recursive=true 一次性列出所有结构" ← 知道该广度优先
- thought 2: "It's clearly a Cocos Creator project... Let me explore the keyfiles" ← 拿到列表后立刻转向读文件

**结论**：agent 知道该广度优先，但固有倾向是拿到结构后钻细节。prompt 能引导轮 1 做对，但无法阻止后续转向深度。

## 待验证问题（积累样本后再决定）

1. **「深度优先」是否在多个不同工程上反复导致问题？**
   - 如果只是「读了比理想多几个文件但最终结果正确」→ 可接受，不优化
   - 如果反复导致「读太多无关文件、总是慢」→ 考虑针对性优化

2. **prompt 对探索深度的约束力边界在哪？**
   - thought 证明「知道但做不到」，纯文字 prompt 边际收益递减
   - 若要更强约束，应走机制层（如探索调用次数硬限制），而非改 prompt 措辞

3. **是否需要让模型重写 prompt（元优化）？**
   - 现阶段不推荐：样本不足（仅 1 个成功案例），手写领域 prompt 优于模型自生成
   - 真正有效的元优化是迭代式：用 1-2 周 → 收集失败案例 → 针对性调整

## 决策记录（2026-06-18）

**不专门为「深度优先」优化 prompt。** 理由：
- 14 轮完成、未失败，是质的改善，再压轮次是边际收益递减
- prompt 对「探索深度」约束力有限（thought 证明知道但做不到）
- 为单一场景定制 prompt 有过拟合、干扰其他任务的风险

**下一步：保持现状，积累更多样本。** 用 1-2 周观察不同任务类型（整理架构、写功能、改 bug、重构）的实际表现，若「深度优先」反复导致问题再针对性优化。

## 相关代码位置（供后续参考）

- system_prompt 工作方式规则：`src-tauri/src/core/agent.rs:97-107`
- MAX_ROUNDS：`src-tauri/src/core/agent.rs:29`
- 轮次预算提醒（REMINDER_THRESHOLD=5）：`src-tauri/src/core/agent.rs` 主循环顶部
- OVERALL_TIMEOUT：`src-tauri/src/core/agent.rs:182`
- 无活动超时（STREAM_IDLE_TIMEOUT=60s）：`src-tauri/src/core/provider.rs` 流式循环
- reqwest connect_timeout：`src-tauri/src/core/provider.rs:142-146`
- list_files 关键文件摘要：`src-tauri/src/core/tool_spec.rs` ListFilesTool::dispatch
- is_key_file / KEY_FILE_NAMES：`src-tauri/src/core/tool_spec.rs` 常量区
