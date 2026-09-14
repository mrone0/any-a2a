# Pi remote A2A subagents

Pi extension for the locally installed `@earendil-works/pi-coding-agent` API. This is remote delegation through a custom tool, not an LLM provider and not a replacement for Pi's example `subagent` extension.

## Install / try (PowerShell, repository root)

Build the native CLI first. Close the desktop app if Windows has the executable locked.

```powershell
cargo build
$env:ANY_A2A_EXECUTABLE = (Resolve-Path .\target\debug\any-a2a.exe).Path
# Optional: must be absolute; omit to use the platform's any-a2a data directory.
# $env:ANY_A2A_DATA_DIR = 'C:\Users\admin\AppData\Local\any-a2a'
pi -e .\adapters\pi\index.ts
```

For persistent extension installation: `pi install ./adapters/pi`, then `/reload` in Pi. The executable setting is a deployment environment variable: set it in the shell launching Pi, or put the native CLI on PATH. Do not pass it as a model-controlled tool argument. Installing does not copy the CLI or adapter source; keep their paths valid. No user Pi configuration is modified by merely adding this package to the repository.

## Tools

- `a2a_agents`: read the current saved catalog and return IDs, descriptions and skills. Stored auth fields are not returned.
- `a2a_delegate`: `{agentId, task}`; starts a client-owned native CLI subprocess with `--events`. Standalone input only; does not forward parent conversation, local tools, persona or model credentials. Pi's normal parallel tool calls may run independent delegations concurrently.

Example prompt: “Use a2a_agents to find the temporary verification specialist, then invoke a2a_delegate once to demonstrate simulated progress. Do not substitute a local subagent or generate a success marker yourself.”

Live protocol/state/remote progress is delivered with Pi's supported `onUpdate`. Final text is returned as readable text rather than a nested JSON string. Full final A2A outcome and bounded progress are persisted in the standard tool result `details`, with a version field. No invented session event types, assistant/model messages, reasoning blocks, or usage accounting. Final model-visible tool content also includes the retained progress timeline, so completion/reload does not replace all intermediate information with only the final answer. Receiving progress does not certify real-time TUI rendering; that requires UI acceptance. Pi's default tool renderer is used. Each invocation now has an independent adapter-owned run ID and disk record under `<Pi agent dir>/a2a-runs/<hashed parent session>/`. These are not native Pi child model sessions. No custom background-job panel is implemented.

## Remote subagent run management

`a2a_delegate` accepts optional `background: true`. It returns a run ID immediately instead of waiting. Use `a2a_runs` with no arguments to list the current parent session's runs, or `{runId}` to inspect progress/result. `a2a_stop` stops a live local run owned by that parent; remote cancellation is not implied. Generic `subagent` tools are not overridden.

Records include owner, agent ID, task, state, timestamps, retained progress and final outcome/error. Running progress is available from memory; initial and terminal snapshots are saved atomically. Crash-time intermediate progress is not guaranteed durable. On session start, orphaned running records become interrupted without resubmitting. Completed records remain readable through a2a_runs after reload. Max four live runs (per extension instance), 200 records per parent, 10 MiB per record; archive old record files manually with Pi stopped. Records include private task/output data and inherit the user directory's Windows ACL.

Examples:

```text
a2a_delegate({agentId: "saved-id", task: "standalone task", background: true})
a2a_runs({runId: "returned-uuid"})
a2a_stop({runId: "returned-uuid"})
```

See the repository-wide `docs/client-subagent-contract.md` for client integration completion criteria. Desktop one-click installation, native child sessions, dedicated TUI panels, remote cancel, and background UI acceptance remain outstanding.

## Limits and lifecycle

- Model-visible output: 50 KiB / 2000 lines; truncation explicitly reported. Full final output remains in `details`.
- CLI stdout: 8 MiB maximum, 125-second execution deadline; catalog 30 seconds.
- Retained progress: 100 messages / 50,000 UTF-8 bytes, per remote message 8,000 characters. Successful polling chatter is omitted; repeated status messages are deduplicated. Limits are reported via `progressTruncated` (individual per-message cap applies separately).
- Cancellation or session shutdown kills and awaits the direct CLI subprocess. **This stops local waiting, NOT remote work.** No remote cancellation confirmation or task recovery is promised. No automatic retry. Use the native executable, not a process-spawning wrapper.
- Remote status and final output are untrusted data; simulations remain labeled. No automatic local tool execution based on remote output.
- Prompts are process arguments and may be visible to local process inspection. Catalog credentials are read by the CLI and not copied into the Pi extension configuration. Ambient env names containing KEY/SECRET/TOKEN/PASSWORD are removed, including ANY_A2A_TOKEN: catalog mode uses saved auth.
- Factory load starts no background work. Existing DSH adapters and desktop integration are not changed. Desktop one-click Pi installation is not implemented yet.

## Tests

```powershell
$env:PI_TEST_ROOT = 'C:\Users\admin\AppData\Roaming\npm\node_modules\@earendil-works\pi-coding-agent'
# Build path used by the test; override with ANY_A2A_TEST_BINARY if needed.
cargo build --target-dir target/demo-build
npm --prefix adapters/pi test
```

Tests load the extension with the real installed Pi loader, call its registered tool against a local HTTP fixture through the real Rust CLI, check auth isolation, live progress, results, abort, and reload SDK-generated tool-result entries with SessionManager.open. Without PI_TEST_ROOT, the installed-Pi integration test is skipped explicitly. This is not yet a real parent-model/TUI acceptance test.
