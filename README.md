# Sophoni

Sophoni 是一个桌面端优先的工作区 AI Agent。当前仓库处于 MVP 基础骨架阶段。

## 技术栈

- Tauri 2
- Svelte
- TypeScript
- Rust
- SQLite

## 本地开发

```bash
pnpm install
pnpm dev
```

运行桌面端：

```bash
pnpm tauri dev
```

运行测试：

```bash
pnpm check
pnpm test
cargo test --manifest-path src-tauri/Cargo.toml
```

## 当前能力

- 三栏桌面工作台。
- Rust Core Runtime 基础领域模型。
- SQLite schema 骨架。
- 工作区文件读写和 diff。
- 命令风险分类。
- mock Agent 任务流。

## 尚未实现

- GLM Provider 真连接入。
- 真实 Function Calling 工具循环。
- 高级结构索引。
- 真实命令执行器。
- macOS Keychain 设置页。
