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

## 本版协议范围

- 明确支持 **A2A 0.3.0 JSON-RPC**：Agent Card、`message/send`、`tasks/get`。
- 支持直接 Message 结果和 Task 轮询，读取文本 artifact／状态消息。
- 支持 Bearer token；HTTP 重定向禁用；Card 与实际 endpoint 必须同源，避免意外转发密钥或任务。
- 单响应上限 4 MiB；HTTP 请求最多 30 秒；任务有总超时。
- 不支持 A2A 1.0、SSE、gRPC、OAuth、多模态文件传输、任务恢复、双向补充输入。
- `input-required` / `auth-required` 明确报错，不伪装成完成。
- **杀死本机进程或超时不会取消远程任务。** 本版未实现远程取消；请只对可控测试任务使用。
- 同源校验不是完整 SSRF 沙箱：URL 由可信用户配置；本版不开放远程配置 API。
- `--message` 参数可能被本机进程查看工具看到；暂不用于敏感材料。

## 验证边界

Rust 自动化测试通过本地 HTTP fixture 验证真实 HTTP/JSON-RPC 编解码：直接结果、长任务轮询、暂停状态、跨源拒绝、版本拒绝。

DSH 上游参考源码放在 `.research/deepseek-harness`（gitignore，不分发），固定检查版本：`c291e7961a515f6d7af9304e7fd1d257929aef26`。

**本地 fixture 不等于第三方服务兼容性认证。** 真实用户模型会话与真实远程 Agent 还需提供可用环境进行验收。
