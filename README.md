# any-a2a — DSH 可行性验证版

Rust A2A 客户端 + DeepSeek Harness 子 Agent provider 薄适配层。**不使用 MCP，不需要 skill，不捆绑 Node.js 运行时**。DSH 本身是 TypeScript 宿主，适配层由宿主运行；协议通信在 Rust 二进制中完成。

本版本验证一条路径：DSH 委派 → provider → Rust CLI → A2A 服务 → 结果交回 DSH。不是桌面 GUI，也不是完整的 A2A 产品。

## 构建与检查

```sh
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

二进制：`target/release/any-a2a`。本机首次 release 构建约 2.0 MiB（仅可执行文件；不是源码、依赖和构建缓存总大小）。选择体积优化，不宣称所有场景最快。

### Windows 本地桌面端构建

需要 Rust MSVC 工具链、Visual Studio Build Tools 的「使用 C++ 的桌面开发」（含 Windows SDK），运行桌面端还需要 WebView2 Runtime。

在仓库根目录执行：

```powershell
cargo build
cargo build --manifest-path desktop/src-tauri/Cargo.toml
.\desktop\src-tauri\target\debug\any-a2a-desktop.exe
```

第一步生成桌面端默认启动的本地服务。桌面端 Windows 资源编译需要 `desktop/src-tauri/icons/icon.ico`；该文件随源码提交，并在 `tauri.conf.json` 的 `bundle.icon` 中声明。上述命令构建本地调试程序，不生成安装包。

## 单独使用

```sh
./target/release/any-a2a run \
  --card http://127.0.0.1:8080/.well-known/agent-card.json \
  --message '请审查这个方案' \
  --timeout-secs 120
```

私有服务可设置 `ANY_A2A_TOKEN`（Bearer token）；不要把密钥写进仓库或提示词。成功时 stdout 为一行 JSON：

```json
{"text":"结果","task_id":"task-1","context_id":"context-1","state":"completed"}
```

失败时返回非零退出码，诊断写入 stderr。不会自动重试消息提交，避免重复触发有副作用的任务。

DSH 安装／验证说明见 [adapters/dsh](adapters/dsh/README.md)。

桌面端新生成的 DSH 配置采用 **client → CLI → 远端 Agent**，不再经过桌面 App 的临时 service。关闭 App 不影响这些客户端调用。`any-a2a run --agent-id ID --message TEXT` 从本地目录读取该 Agent 和认证；`any-a2a catalog` 输出不含存储认证字段的目录。已有 serviceUrl/serviceToken 配置需备份、移除旧的 any-a2a 条目后重新应用，不会自动迁移。客户端停止 CLI 不等于取消远端任务；桌面取消按钮只管理桌面测试请求。

## 本版协议范围

- 明确支持 **A2A 0.3.0 JSON-RPC**：Agent Card、`message/send`、`tasks/get`。
- 支持直接 Message 结果和 Task 轮询，读取文本 artifact／状态消息。
- 支持 Bearer token；HTTP 重定向禁用；Card 与实际 endpoint 必须同源，避免意外转发密钥或任务。
- 单响应上限 4 MiB；HTTP 请求最多 30 秒；任务有总超时。
- 不支持 A2A 1.0、SSE、gRPC、OAuth、多模态文件传输、任务恢复、双向补充输入。
- `input-required` / `auth-required` 明确报错，不伪装成完成。
- **杀死本机进程或超时不会取消远程任务。** 桌面运行测试支持显式取消：A2A 0.3 使用 `tasks/cancel`，1.0 使用 `CancelTask`。只有远端返回匹配任务 ID 和 canceled 状态才确认取消；失败不自动重试。
- 取消是协作式的：当前 HTTP 请求返回并取得任务 ID 后才能发送取消（单请求最多 30 秒）。如果 SendMessage 超时且未取得任务 ID，无法取消未知的远端任务。仅支持当前仍在运行的本地请求，不支持重启后恢复取消。
- 本地 API：`POST /api/run` 可携带唯一 `runId`，另一请求调用 `POST /api/cancel`（`{"runId":"..."}`，相同本地 Bearer 认证）。取消接口返回 requested 仅表示本地接受意图，最终以 run 响应为准；若运行尚未登记或已经结束，取消接口会报错。
- 同源校验不是完整 SSRF 沙箱：URL 由可信用户配置；本版不开放远程配置 API。
- `--message` 参数可能被本机进程查看工具看到；暂不用于敏感材料。

## 验证边界

Rust 自动化测试通过本地 HTTP fixture 验证真实 HTTP/JSON-RPC 编解码：直接结果、长任务轮询、暂停状态、跨源拒绝、版本拒绝。

DSH 上游参考源码放在 `.research/deepseek-harness`（gitignore，不分发），固定检查版本：`c291e7961a515f6d7af9304e7fd1d257929aef26`。

**本地 fixture 不等于第三方服务兼容性认证。** 真实用户模型会话与真实远程 Agent 还需提供可用环境进行验收。
