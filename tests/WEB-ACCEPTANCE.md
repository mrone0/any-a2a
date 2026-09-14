# Local Windows Web acceptance

Verified against `D:/code/deepseek-harness` commit `b150a551b8d465e31e418e1b2eaf5e79bbb7d28e`, using the real `pnpm dsh web` entry, local model configuration, and headless Microsoft Edge through Playwright. No upstream source modified.

## Fixes

- Actual Web profile had no any-a2a entries. Installed the temporary provider and custom delegation tool, with `file:///` module URLs and the new CLI in `target/demo-build/debug/any-a2a.exe`.
- Backed up `~/.dsh/profiles/web/cordis.patch.yml` before modification. No tokens copied into the patch.
- Remote delegation input now uses a user-message source; plugin source was folded into Web's context-injection UI.
- Raw remote results use a JSON code block.
- Capability instructions prohibit substituting a local spawn agent for the remote tool.

## Evidence

Old child `e1d980ab-c028-42b7-8a78-754c6423ba7e` was a local `spawn` child, not A2A. Its parent invoked generic `subagent` and instructed it to output the test marker. This is not remote-call evidence.

New Web child `e2308dc1-81a9-4f53-bdda-60636e7b0cdb` was created through actual `a2a_demo` invocation. Browser navigation: workspace `diy-pc` -> session `一次调用验证输入可见` -> `1 个子代理` -> `a2a_demo: 输入显示验收`.

Browser assertions passed for visible input `WEB-INPUT-CHECK 验证输入可见`, `message/send`, working/completed states, and `A2A-DEMO-OK`. The complete page was captured in the local temporary demo directory as `web-child-fixed.png`. This proves protocol events and remote output, not internal remote thinking or tool execution. The fixture is deterministic and loopback-only.

The separate test Web instance used port 63530; the user's existing Web process was not terminated. The new configuration takes effect on restart of that process. Previous session logs are not rewritten. Temporary profile configuration depends on the demo executable path and the running fixture; it is not a permanent deployment.
