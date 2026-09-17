# any-a2a

**any-a2a** 是一个面向客户端的开源远程 Subagent 委派工具：它把本地客户端中的一次委派请求，通过 Rust CLI 转换为 A2A（Agent2Agent）协议请求，发送给远端 Agent，再将结果安全地带回原客户端。

> 当前版本：`0.1.0-alpha.1`（预览版）
>
> 本项目仍处于可行性验证阶段。它不是完整的 A2A 平台，也不是 LLM provider；桌面端主要负责配置、目录管理和测试，真正的任务执行由调用它的客户端和 Rust CLI 负责。

## 开源声明

本项目以 **MIT License** 开源，完整许可文本见 [LICENSE](LICENSE)。你可以自由使用、修改、复制和分发本项目，但须保留版权和许可声明。

项目中使用的 Rust crate、Node.js 包、桌面框架以及各客户端宿主均遵循其各自的许可证；它们不因本项目采用 MIT License 而改变。远端 Agent、模型服务和用户提交的提示词/结果不属于本仓库的开源内容，使用时请分别遵守对应服务的条款与隐私政策。

## 工作原理

any-a2a 将「客户端生命周期」和「A2A 网络通信」分开：

```text
客户端主 Agent
    │  原生委派入口（Pi tool / DSH provider）
    ▼
any-a2a CLI（本地独立进程）
    │  Agent Card + SendMessage/GetTask（A2A 1.0 JSON-RPC）
    ▼
远端 A2A Agent
    │  直接 Message 或 Task 轮询结果
    ▼
客户端结果 / 运行记录
```

1. 客户端通过受支持的适配层发现本地保存的 Agent，或提供 Agent Card URL。
2. CLI 读取 Agent Card，校验协议版本、同源 endpoint 和必要的认证信息。
3. CLI 向远端 Agent 发送 `SendMessage`（A2A 1.0）或 `message/send`（A2A 0.3.0）JSON-RPC 请求。远端可以直接返回 Message，也可以返回 Task。
4. 对于 Task，CLI 按策略调用 `GetTask`（A2A 1.0）或 `tasks/get`（A2A 0.3.0）轮询，直到任务完成、失败、暂停或超时。
5. CLI 输出一个结构化 JSON 结果；适配层把它映射为客户端自己的结果和生命周期状态，并持久化有界的运行记录。

CLI 是独立的执行边界：桌面 App 退出不会中断客户端自己启动的 CLI 调用。客户端停止本地进程也**不代表**远端任务已经取消；只有远端返回匹配任务 ID 且状态为 `canceled`，才能确认远端取消。项目不会自动重试消息提交，以免重复触发有副作用的远程任务。

## 特性与安全边界

- 支持 A2A `1.0` JSON-RPC 绑定：PascalCase 方法 `SendMessage`/`GetTask`/`CancelTask`、`A2A-Version: 1.0` 请求头、`ROLE_USER`/`TASK_STATE_*` 枚举，以及 Card 的 `supportedInterfaces` 接口选择（含 `tenant` 透传）。
- 兼容 A2A `0.3.0` JSON-RPC：`message/send`/`tasks/get`/`tasks/cancel` 和旧版 Card 字段（`url`/`preferredTransport`/`protocolVersion`）。
- 支持直接 Message 结果和 Task 轮询，读取文本 artifact 与状态消息。
- 支持 Bearer token；HTTP 重定向禁用，Card 与实际 endpoint 必须同源。
- 对 HTTP 请求、响应体、CLI 输出、进度和持久化记录设置大小/时间上限。
- 远端状态、进度和最终输出均视为不可信数据，不会根据远端输出自动执行本地工具。
- A2A 1.0 仅实现 JSON-RPC 绑定的核心操作；不支持 `SendStreamingMessage`/`SubscribeToTask`（SSE 流式）、`ListTasks`、push notification、gRPC 与 HTTP+REST 绑定、OAuth、多模态文件传输、任务恢复和双向补充输入。
- `input-required` / `auth-required` 会明确报错，不会伪装成完成。
- 同源校验不是完整 SSRF 沙箱；URL 来自可信的本地配置，本版不开放远程配置 API。
- `--message` 会作为命令行参数传递，可能被本机进程查看工具看到；敏感认证信息应使用环境变量或本地保存的 Agent 配置，不要写进仓库、URL 或提示词。

## 构建与检查

需要 Rust stable（Edition 2024）。

```sh
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

二进制为 `target/release/any-a2a`。Release profile 使用体积优化、LTO、单代码生成单元和 strip；这并不代表所有场景下都是最快的实现。

### Windows 本地桌面端

需要 Rust MSVC 工具链、Visual Studio Build Tools 的「使用 C++ 的桌面开发」（含 Windows SDK），运行桌面端还需要 WebView2 Runtime。

```powershell
cargo build
cargo build --manifest-path desktop/src-tauri/Cargo.toml
.\desktop\src-tauri\target\debug\any-a2a-desktop.exe
```

上述命令构建本地调试程序，不生成安装包。Windows 资源文件 `desktop/src-tauri/icons/icon.ico` 已随源码提交。

## CLI 使用

直接指定 Agent Card：

```sh
./target/release/any-a2a run \
  --card http://127.0.0.1:8080/.well-known/agent-card.json \
  --message '请审查这个方案' \
  --timeout-secs 120
```

私有服务可设置 Bearer token：

```sh
export ANY_A2A_TOKEN='your-token'
./target/release/any-a2a run --card 'https://agent.example/.well-known/agent-card.json' --message '...' \
  --timeout-secs 120
```

不要把 token 写入仓库或提示词。成功时 stdout 输出一行 JSON：

```json
{"text":"结果","task_id":"task-1","context_id":"context-1","state":"completed"}
```

失败时进程返回非零退出码，诊断写入 stderr。

桌面端生成的客户端配置采用：

```text
client → any-a2a run --agent-id ID → remote Agent
```

`--agent-id` 从本地目录读取 Agent 和认证信息；`any-a2a catalog` 输出不含认证字段的 Agent 目录。已有 `serviceUrl/serviceToken` 配置不会自动迁移，迁移前请备份并只移除旧的 any-a2a 配置项。

## 客户端适配层

| 客户端 | 说明 | 状态 |
| --- | --- | --- |
| [Pi](adapters/pi/README.md) | 使用 Pi 公共 Extension API、工具调用和 adapter-owned run records | 已实现，仍需真实 TUI 验收 |
| [DeepSeek Harness / DSH](adapters/dsh/README.md) | 使用 DSH provider/JobOutcome，将一次远程调用映射为 one-shot subagent | 已实现，须使用文档中的固定版本 |
| [OpenCode](adapters/opencode/README.md) | 原生 task 的探索性集成 | 原型，未完成发布验收 |

适配层不是通用协议模拟器：每个客户端的生命周期、持久化、取消和 UI 能力都以对应 README 中的实际验证范围为准。不要把 any-a2a 的 adapter-owned 运行记录误认为客户端原生可恢复的子 Agent session。

DSH 的安装与验证说明见 [adapters/dsh](adapters/dsh/README.md)；Pi 的安装、后台运行和记录管理见 [adapters/pi](adapters/pi/README.md)。

## 取消、持久化与限制

- 本地 abort、超时或杀死 CLI 只停止本地等待，不自动取消远端工作。
- A2A 远端取消只能在已取得任务 ID 后通过 `CancelTask`（A2A 1.0）或 `tasks/cancel`（A2A 0.3.0）请求，并以远端确认的 `canceled` 状态为准。
- 本地 API 的 `POST /api/run` 支持唯一 `runId`，`POST /api/cancel` 只表示本地接受取消意图；最终状态以运行结果为准。
- 运行记录与客户端 parent session 关联；恢复时不会重新提交任务。崩溃期间的中间进度不保证持久化。
- 适配层会限制输出、并发、记录数量和进程生命周期；完整的有界结果保存在 tool details 或运行记录中，模型上下文只接收可读摘要。

## 验证范围

Rust 测试使用本地 HTTP fixture 验证真实 HTTP/JSON-RPC 编解码，包括直接结果、长任务轮询、暂停状态、跨源拒绝和版本拒绝。客户端测试还覆盖真实 CLI、认证隔离、进度、结果、abort、reload、缺失 Agent 和异常输出。

本地 fixture 不等于第三方服务兼容性认证。真实模型会话、真实远端 Agent、客户端完整 UI、Windows 安装包和跨客户端任务恢复仍需在目标环境单独验收。上游 DSH 参考源码位于 `.research/deepseek-harness`（gitignored），使用的固定检查版本及限制见 [DSH adapter 文档](adapters/dsh/README.md)。

## 贡献与反馈

欢迎提交 Issue 和 Pull Request。涉及客户端集成时，请先阅读 [客户端 Subagent 集成契约](docs/client-subagent-contract.md)，特别是远端取消、持久化归属、真实客户端渲染和“未实现能力必须明确标记”的要求。请不要提交 token、私有 Agent Card、模型密钥或运行记录。
