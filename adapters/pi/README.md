# Pi remote A2A subagents

Pi extension for the locally installed `@earendil-works/pi-coding-agent` API. This is remote delegation through a custom tool, not an LLM provider and not a replacement for Pi's example `subagent` extension.

Desktop-managed installation embeds the adapter and copies it together with the native CLI into an immutable version directory. This route does not need a source checkout at runtime. On Windows, desktop invokes the installed Pi npm package through its native Node.js entry point; the locally inspected host version is Pi `0.85.1`. The source-based commands below remain available for development.

## Install / try (PowerShell, repository root)

Build the native CLI first. Close the desktop app if Windows has the executable locked.

```powershell
cargo build
$env:ANY_A2A_EXECUTABLE = (Resolve-Path .\target\debug\any-a2a.exe).Path
# Optional: must be absolute; omit to use the platform's any-a2a data directory.
# $env:ANY_A2A_DATA_DIR = Join-Path $env:LOCALAPPDATA 'any-a2a'
pi -e .\adapters\pi\index.ts
```

For persistent extension installation: `pi install ./adapters/pi`, then `/reload` in Pi. The executable setting is a deployment environment variable: set it in the shell launching Pi, or put the native CLI on PATH. Do not pass it as a model-controlled tool argument. Installing does not copy the CLI or adapter source; keep their paths valid. No user Pi configuration is modified by merely adding this package to the repository.

## Tools

- `a2a_agents`: read the current saved catalog and return IDs, descriptions and skills. Stored auth fields are not returned.
- `a2a_delegate`: `{agentId, task}`; starts a client-owned native CLI subprocess with `--events`. Standalone input only; does not forward parent conversation, local tools, persona or model credentials. Pi's normal parallel tool calls may run independent delegations concurrently.

Example prompt: “Use a2a_agents to find the temporary verification specialist, then invoke a2a_delegate once to demonstrate simulated progress. Do not substitute a local subagent or generate a success marker yourself.”

Live protocol/state/remote progress is delivered with Pi's supported `onUpdate`. Final text is returned as readable text rather than a nested JSON string. Full final A2A outcome and bounded progress are persisted in the standard tool result `details`, with a version field. No invented session event types, assistant/model messages, reasoning blocks, or usage accounting. Final model-visible content reports the retained progress count and truncation; the detailed timeline remains in `details` and the run record. Receiving progress does not certify real-time TUI rendering; that requires UI acceptance. Pi's default tool renderer is used. Each invocation now has an independent adapter-owned run ID and disk record under `<Pi agent dir>/a2a-runs/<hashed parent session>/`. These are not native Pi child model sessions. No custom background-job panel is implemented.

## Remote subagent run management

`a2a_delegate` runs in the background by default and returns a run ID after local startup. Set `background: false` to wait in the foreground. Use `a2a_runs` with no arguments to list the current parent session's runs, or `{runId}` to inspect progress/result. `a2a_stop` stops a live local run owned by that parent; remote cancellation is not implied. Generic `subagent` tools are not overridden.

Records include owner, agent ID, task, state, timestamps, retained progress and final outcome/error. Running progress is available from memory; initial and terminal snapshots are saved atomically. Crash-time intermediate progress is not guaranteed durable. On session start, orphaned running records become interrupted without resubmitting. Completed records remain readable through a2a_runs after reload. Max four live runs (per extension instance), 200 records per parent, 20 MiB per record; archive old record files manually with Pi stopped. Records include private task/output data and inherit the user directory's Windows ACL.

The four-run limit includes pending catalog checks and initial saves. Cancellation stays linked to the caller through startup; a successful background response transfers it to the owning session. Startup errors after an initial snapshot save a failed/stopped terminal record with an end time. An initial persistence failure prevents submission; a terminal persistence failure reports that the saved state may be stale and must not be used to justify resubmission. Shutdown aborts and awaits pending and live work, and a subsequent `session_start` permits new calls.

Examples:

```text
a2a_delegate({agentId: "saved-id", task: "standalone task", background: true})
a2a_runs({runId: "returned-uuid"})
a2a_stop({runId: "returned-uuid"})
```

See the repository-wide `docs/client-subagent-contract.md` for client integration completion criteria. Native child sessions, remote cancel, and real TUI/background UI acceptance remain outstanding. Desktop installation deployment has automated checks; installation on target machines still requires acceptance.

## Limits and lifecycle

- Model-visible catalog: 50 KiB / 2000 lines. Foreground delegation and run inspection: 8 KiB / 120 lines. Truncation is explicit; full bounded final output remains in `details` and the run record.
- CLI stdout: 20 MiB maximum, 16 MiB final frame, 16 KiB progress frame, 125-second execution deadline; catalog 30 seconds. Producer progress is limited separately so large evolving status messages do not consume the final result budget. `progress_truncated` reports producer truncation.
- Retained progress: 100 messages / 50,000 UTF-8 bytes, per remote message 8,000 characters. Successful polling chatter is omitted; repeated status messages are deduplicated. Limits are reported via `progressTruncated` (individual per-message cap applies separately).
- Cancellation or session shutdown kills and awaits the direct CLI subprocess. **This stops local waiting, NOT remote work.** No remote cancellation confirmation or task recovery is promised. No automatic retry. Use the native executable, not a process-spawning wrapper.
- Remote status and final output are untrusted data; simulations remain labeled. No automatic local tool execution based on remote output.
- Prompts are process arguments and may be visible to local process inspection. Catalog credentials are read by the CLI and not copied into the Pi extension configuration. Ambient env names containing KEY/SECRET/TOKEN/PASSWORD are removed, including ANY_A2A_TOKEN: catalog mode uses saved auth.
- Factory load starts no background work. Desktop installation stages a persistent adapter/CLI bundle and invokes the host's supported package installer, with a backup of existing Pi settings.

## Tests

```powershell
$env:PI_TEST_ROOT = Join-Path $env:APPDATA 'npm\node_modules\@earendil-works\pi-coding-agent'
# Build path used by the test; override with ANY_A2A_TEST_BINARY if needed.
cargo build --target-dir target/demo-build
npm --prefix adapters/pi test
```

Tests load the extension with the actual Pi `0.85.1` loader and public `ExtensionRunner`, call registered tools against a local HTTP fixture through the native Rust CLI, and verify parallel admission, background handoff, startup failures, shutdown, owner checks, and persisted reload. Each test isolates `PI_CODING_AGENT_DIR` in its temporary fixture. Pure lifecycle tests additionally exercise pending-save cancellation, retention claims, persistence failure, and terminal notification failure. Without `PI_TEST_ROOT`, installed-Pi tests are explicitly skipped. These are loader/runner and storage checks; real parent-model/TUI acceptance remains outstanding.
