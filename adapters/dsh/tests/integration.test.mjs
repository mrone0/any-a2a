// This is real source-level service composition, not a DSH Loader/profile e2e.
// Run: node --import ./tests/dsh-loader.mjs --test ./tests/integration.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { Context } from '@deepseek-ai/cordis';
import SessionProjectionRegistry from '@deepseek-ai/dsh-session-projection';
import { SubagentRuntime } from '@deepseek-ai/dsh-subagent';
import * as adapter from '@any-a2a/dsh';

const config = {
  cardUrl: 'http://127.0.0.1:1/.well-known/agent-card.json',
  executable: fileURLToPath(new URL('./fixture-cli.mjs', import.meta.url)),
};

async function mount(t) {
  const ctx = new Context();
  t.after(() => ctx.fiber.dispose());
  await ctx.plugin(SessionProjectionRegistry);
  await ctx.plugin(SubagentRuntime);
  const fiber = await ctx.plugin(adapter, config);
  assert.ok(ctx.subagents instanceof SubagentRuntime);
  assert.ok(ctx.sessionProjections instanceof SessionProjectionRegistry);
  assert.deepEqual(ctx.subagents.list(), ['any-a2a']);
  return { ctx, fiber };
}

function request(text = 'hello') {
  return {
    prompt: [{ type: 'text', text }],
    // Out-of-process one-shot delegation uses only the parent's scope identity;
    // no Agent service or local child is fabricated or mounted.
    parent: { id: 'integration-parent' },
    signal: new AbortController().signal,
  };
}

test('actual DSH runtime dispatches hello through the Cordis-mounted CLI provider', { timeout: 10000 }, async t => {
  const { ctx, fiber } = await mount(t);
  const starts = [];
  const ends = [];
  const removed = [];
  ctx.on('subagent/start', info => { starts.push(info); });
  ctx.on('subagent/end', info => { ends.push(info); });
  ctx.on('subagent/provider-removed', name => { removed.push(name); });
  const run = await ctx.subagents.start('any-a2a', request());
  t.after(() => run.dispose());
  assert.equal(run.localAgent, undefined);
  assert.equal(typeof run.id, 'string');
  assert.deepEqual(await run.result, {
    output: [{ type: 'text', text: 'hello' }],
    stopReason: 'completed',
  });
  await Promise.resolve();
  assert.equal(starts.length, 1);
  assert.equal(ends.length, 1);
  assert.equal(starts[0].provider, 'any-a2a');
  assert.equal(ends[0].runId, starts[0].runId);
  assert.equal(ends[0].stopReason, 'completed');
  assert.deepEqual(ends[0].lastAssistantMessage, [{ type: 'text', text: 'hello' }]);
  await run.dispose();
  await fiber.dispose();
  assert.deepEqual(removed, ['any-a2a']);
  assert.equal(ctx.subagents.getProvider('any-a2a'), undefined);
  await assert.rejects(ctx.subagents.start('any-a2a', request()), { code: 'NO_PROVIDER' });
});

test('actual DSH runtime rejects unsupported capabilities before CLI dispatch', { timeout: 10000 }, async t => {
  const { ctx } = await mount(t);
  const starts = [];
  ctx.on('subagent/start', info => { starts.push(info); });
  await assert.rejects(ctx.subagents.start('any-a2a', {
    ...request(), persona: 'reviewer',
  }), { code: 'UNSUPPORTED_CAPABILITY' });
  assert.deepEqual(starts, []);
});

for (const message of ['nonzero', 'invalid']) {
  test(`actual DSH runtime observes ${message} CLI failure as a settled error`, { timeout: 10000 }, async t => {
    const { ctx } = await mount(t);
    const ends = [];
    ctx.on('subagent/end', info => { ends.push(info); });
    const run = await ctx.subagents.start('any-a2a', request(message));
    t.after(() => run.dispose());
    const result = await run.result;
    assert.equal(result.stopReason, 'error');
    assert.deepEqual(result.output, []);
    assert.doesNotMatch(JSON.stringify(result), /secret-token|private-prompt/);
    await Promise.resolve();
    assert.equal(ends.length, 1);
    assert.equal(ends[0].stopReason, 'error');
  });
}
