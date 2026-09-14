# Cross-client remote subagent delegation contract

## Architecture

`client main agent -> client-native delegation entry -> any-a2a CLI -> remote A2A agent`

Desktop is a control/configuration surface, not the execution owner. No client may require a running desktop to submit, observe, or collect its own task. Remote work may outlive local processes; this does not imply local result collection survives them.

## Common run model

Every adapter-managed delegation has a unique local run ID, owning parent session ID, selected catalog agent ID, task text, start/update/end times, local state, remote task/context IDs when available, progress, and final outcome or safe diagnostic. Remote task ID, client session ID, and local run ID are distinct.

Minimum local states: running, completed, failed, stopped, interrupted. Remote canceled is recorded separately as confirmed remote state. On cold start, persisted running without a live owner is interrupted, never completed/canceled. Reopening a record never resubmits a message.

Background delegation returns its run ID promptly, supports status/result collection and explicit local stop, and leaves the main agent free to continue. Foreground delegation uses the same run model. Ownership checks apply to every inspect/stop request; no arbitrary file paths or executable names are model-controlled arguments.

## Presentation and persistence

Expose agent name/ID, task, status, elapsed time, progress and final output. Use the client's native child session facilities if public and suitable. Otherwise provide an explicit adapter-owned run panel/tools and versioned records; do not claim these are native resumable model sessions.

Only authoritative remote content can supply progress, tool events, or reasoning summaries. Simulated events remain labeled. Protocol polling is not remote thought. Merge repetitive successful polling. Persist final tool details through host APIs; never manufacture assistant/model events to satisfy validators.

Use bounded readable summaries in model context, with explicit truncation. Keep full bounded results in details/run storage. Record schema versions, validate restored files, atomically replace snapshots, and retain old runs across UI reload. Define retention and cleanup.

## Client-specific mapping

- Pi: extension tools + onUpdate; adapter-owned run records associated with Pi session ID. Do not overwrite another extension's `subagent`. Native child session integration is a separate capability, not assumed.
- DSH: provider registry + delegation consumer + jobs; use actual installed JobOutcome and supported session event formats. Test cold child listing/history, not just warm transcript display.
- Claude Code, Codex, OpenCode, Hermes and others: inspect each supported public integration mechanism first. A CLI wrapper, MCP tool, or prompt alone is not native child-session integration. Mark unsupported lifecycle/UI capabilities explicitly. No implementation is implied by appearing in this list.

## Completion checklist (per client/version)

1. Discover saved agents without credentials in output; delegate through actual remote transport.
2. Foreground and background run identity, parent ownership, progress and result collection.
3. Abort/dispose/shutdown and remote-cancel semantics are truthful and tested.
4. Errors, malformed output, caps, unavailable CLI, missing/deleted agents are handled safely.
5. Resume/reload reads completed and interrupted runs without incompatible host events.
6. Real client model calls the correct tool/provider; no impersonation fallback.
7. Client UI shows input, status, progress, result and restart history correctly.
8. Desktop exit does not interrupt client-owned calls.
9. Installation/uninstallation/migration are explicit, backed up and documented.

Partial adapters must list unmet items. Never advertise universal client support based on one adapter passing.
