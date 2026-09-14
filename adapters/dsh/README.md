# any-a2a DSH provider

One-shot **subagent provider**, not an LLM provider, implemented against cloned DeepSeek Harness HEAD `c291e7961a515f6d7af9304e7fd1d257929aef26` (package version `0.1.5-rc.2`). DSH's APIs are pre-stable; other revisions are not verified.

## Install and compose

This standalone ESM package requires no build. Its named `name`, `inject`, and `apply` exports preserve Cordis loader metadata; there is deliberately no default export. Configuration is validated in `apply`, not by an exported Schemastery schema. Registration calls the real effect-scoped `ctx.subagents.registerProvider`; unloading prevents new runs but does not revoke already published runs. Callers own and must dispose those runs.

Install the local package into a DSH profile using the pinned CLI's external bundle mechanism:

```sh
export ANY_A2A_CARD_URL='https://agent.example/.well-known/agent-card.json'
export ANY_A2A_TOKEN='your-token' # optional; never place it in argv
# any-a2a must be on the DSH process PATH
dsh plugin --profile headless add /absolute/path/to/any-a2a/adapters/dsh
```

`package.json` declares `dsh.bundle.patch: ./cordis.patch.yml`; that patch inserts only the Host provider and reads `ANY_A2A_CARD_URL`. The profile must already supply the `subagents` service. In an Agent Preset, independently grant the existing DSH consumer:

```yaml
- name: '@deepseek-ai/dsh-tool-subagent'
  config:
    provider: any-a2a
    toolName: a2a
    maxDepth: provider-managed
    backgroundMode: one-shot
    enableRunInBackground: false
    modelSelectionSettings: false
```

**`maxDepth: provider-managed` is required**: the tool defaults to depth 3, but this remote provider cannot enforce a DSH depth cap. This setting does not impose a remote recursion limit. Keep `backgroundMode: one-shot`; there is no `prepareContinuable`. The package is private/local; the profile-install command is source-derived, not a tested published-package installation.

For explicit composition the plugin config accepts:

| Field | Meaning |
| --- | --- |
| `cardUrl` | Required HTTP(S) card URL, no userinfo. |
| `providerName` | Registry name, default `any-a2a`; use distinct names for multiple cards. |
| `executable` | Executable path or PATH name, default `any-a2a`; not a shell command. |
| `maxOutputBytes` | Positive safe integer; stdout buffer ceiling, default 1 MiB. |

## Execution and security

Each start spawns `any-a2a run --card URL --message TEXT`, without a shell. Text blocks are joined with newlines; non-text blocks and NUL are rejected. No parent transcript, persona, tools, model options, output schema, working-directory context, or delegation depth is transmitted. All five DSH capability flags are false and `inheritsParentContext` is false.

The child receives a copy of the ambient environment with names containing KEY, SECRET, TOKEN, or PASSWORD removed, except `ANY_A2A_TOKEN`, which is explicitly forwarded. The executable is trusted deployment configuration, not sandboxed. This environment filter is not a security sandbox and does not remove every possible credential source. Card URLs and prompts are process arguments, visible to local process inspection; use the token environment variable rather than credential-bearing URLs. Other ordinary environment settings, including proxy settings, remain inherited.

CLI success must be one JSON object containing `text` (string), `task_id` and `context_id` (strings or null), and `state` (string). Only `completed` maps to DSH completion; `canceled`, `rejected`, and `failed` map to aborted/refusal/error. Other states fail without claiming continuation. The supplied Rust CLI already rejects failed/paused states with nonzero exit status. Malformed output, oversized stdout, and nonzero exits become sanitized DSH error results. Stderr is drained but never retained or relayed because DSH forbids raw protocol/credential data in provider diagnostics. Missing executables and pre-publication cancellation reject `start()` after cleanup.

`run.id` is a fresh DSH remote-run identity, **not** an A2A task ID. A2A task/context IDs are validated but not exposed through DSH's one-shot result, which has no metadata slot. No local Agent, durable child session, catalog entry, resumable context, or send-message operation is created.

Abort or `dispose()` forcibly kills the direct CLI process with SIGKILL and waits for its `close` event. Disposal is idempotent. **Local cancellation, timeout, or process kill does not send A2A remote cancellation; the remote task can continue.** There is no adapter-owned timeout or process-tree management. Use the native Rust executable directly, not a wrapper that spawns descendants. HTTP version support, same-origin card endpoint checks, polling and timeout policy belong to the CLI (currently A2A 0.3.0 JSONRPC). There is no token streaming or automatic retry.

## Tests

```sh
# Test-only dependencies stay in this adapter; DSH peers resolve from the clone.
npm --prefix adapters/dsh install --legacy-peer-deps --package-lock=false
npm --prefix adapters/dsh test
node --test adapters/dsh/tests/binary.test.mjs
npm --prefix adapters/dsh run test:dsh
# Entire path: actual DSH registry -> adapter -> compiled Rust CLI -> HTTP fixture
ANY_A2A_TEST_DSH=1 node --import ./adapters/dsh/tests/dsh-loader.mjs --test adapters/dsh/tests/binary.test.mjs
```

Provider tests use an executable fixture, exercise argument transport, state mapping, config/input rejection, credential forwarding, malformed output, spawn failure, real process death, independent runs, and repeated disposal. Binary tests require `target/release/any-a2a` (or `ANY_A2A_TEST_BINARY`), run the compiled Rust CLI against a local HTTP server, and cover immediate messages, task polling, paused-task errors, and bearer auth. No external service or model credentials are needed. The DSH integration test uses the pinned source registry, not a replacement registry; its loader is under `tests/` and verifies HEAD before resolving aliases to cloned source. The minimal parent supplies only its scope identity, as in DSH's own remote provider tests; no model or full Agent application is launched. Test tooling versions are pinned in `devDependencies`. Verified: 8 provider tests, 4 actual registry integration tests, and 3 binary HTTP cases both directly and through the actual registry. Full profile installation/launch, Windows behavior, and TypeScript declaration compilation were not exercised.

## Source authorities / implementation note

Relevant instructions read: cloned root `AGENTS.md` and `docs/defensive-patterns.md`; no cloned files are modified. Implementation is intentionally outside DSH's workspace.

- `packages/subagent/subagent/src/types.ts`: `SubagentProvider`, `SubagentRun`, start-time capabilities, sanitized diagnostics, and optional `prepareContinuable`.
- `packages/subagent/subagent/src/index.ts`: registration effects, capability preflight, resolved descriptors, remote lifecycle handling.
- `packages/subagent/subagent-codex/{src/index.ts,package.json,cordis.patch.yml}`: named plugin exports and Host bundle packaging.
- `packages/subagent/subagent/tests/service.spec.ts`: real Context + SessionProjectionRegistry + SubagentRuntime setup.
- `packages/subagent/tool-subagent/README.md`: one-shot policy and `provider-managed` depth configuration.
- `apps/cli/README.md` and `packages/bundle/README.md`: external profile bundle installation and resolution.

Decision: keep A2A execution in the Rust CLI, and adapt only DSH's one-result, disposable remote-run interface. Do not represent A2A task IDs as DSH sessions or imply continuation support. No custom invariant service is needed: this adapter owns no independently maintained registry or session projection.
