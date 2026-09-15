# OpenCode native A2A subagent (prototype)

Targets OpenCode 1.17.13. Transport remains the Rust CLI. OpenCode's native `task`
creates the parent-linked child session, runs its model, and returns completion.
This is not MCP and does not replace the task tool or synthesize session events.

The plugin registers `any-a2a-remote` (mode subagent), denies all tools except
`a2a_catalog` and `a2a_run`, and requires a parent-linked session before remote
submission. The child adds a model execution; this is intentional isolation.

Native background tasks require `OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS=true`
in this version. No configuration or environment variables are changed automatically.

Status: source prototype only. Tested against installed plugin SDK for registration
and execution guards. Real OpenCode model dispatch, permissions, UI, automatic
completion, abort/reload, Windows, and independent deployment are NOT accepted yet.
No user installation has been modified. Full bounded CLI outcome stays in tool
metadata; model text is limited to 8000 characters. CLI output is bounded at 8 MiB.

Transport currently copies the Pi adapter's bounded CLI transport. Consolidation
into common transport is required before release; npm publication is disabled.
