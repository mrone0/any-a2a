import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { fileURLToPath } from 'node:url'
import * as adapter from '../src/index.js'

const dsh = process.env.ANY_A2A_TEST_DSH === '1' ? {
  Context: (await import('@deepseek-ai/cordis')).Context,
  Projection: (await import('@deepseek-ai/dsh-session-projection')).default,
  Runtime: (await import('@deepseek-ai/dsh-subagent')).SubagentRuntime,
} : undefined

const executable = process.env.ANY_A2A_TEST_BINARY ?? fileURLToPath(new URL('../../../target/release/any-a2a', import.meta.url))
for (const scenario of ['message', 'poll', 'input-required']) {
  test(`compiled Rust CLI -> HTTP fixture: ${scenario}`, { timeout: 10000 }, async () => {
    const methods = []
    const server = createServer(async (req, res) => {
      try {
        assert.equal(req.headers.authorization, 'Bearer fixture-token')
        res.setHeader('content-type', 'application/json')
        if (req.method === 'GET') {
          res.end(JSON.stringify({ protocolVersion: '0.3.0', url: `http://127.0.0.1:${server.address().port}/rpc`, preferredTransport: 'JSONRPC' }))
          return
        }
        const chunks = []
        for await (const chunk of req) chunks.push(chunk)
        const rpc = JSON.parse(Buffer.concat(chunks).toString())
        methods.push(rpc.method)
        if (rpc.method === 'message/send') assert.equal(rpc.params.message.parts[0].text, 'hello')
        else assert.equal(rpc.params.id, 'task-1')
        const result = scenario === 'message'
          ? { kind: 'message', parts: [{ kind: 'text', text: 'answer' }] }
          : { kind: 'task', id: 'task-1', contextId: 'context-1', status: { state: scenario === 'input-required' ? scenario : methods.length === 1 ? 'working' : 'completed' }, ...(methods.length > 1 ? { artifacts: [{ artifactId: 'a1', parts: [{ kind: 'text', text: 'answer' }] }] } : {}) }
        res.end(JSON.stringify({ jsonrpc: '2.0', id: rpc.id, result }))
      } catch (error) {
        server.fixtureError = error
        res.writeHead(500).end()
      }
    })
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
    const previous = process.env.ANY_A2A_TOKEN
    process.env.ANY_A2A_TOKEN = 'fixture-token'
    let run
    let ctx
    try {
      const config = { executable, cardUrl: `http://127.0.0.1:${server.address().port}/.well-known/agent-card.json` }
      const request = { parent: { id: 'binary-test-parent' }, prompt: [{ type: 'text', text: 'hello' }], signal: new AbortController().signal }
      if (dsh) {
        ctx = new dsh.Context()
        await ctx.plugin(dsh.Projection)
        await ctx.plugin(dsh.Runtime)
        await ctx.plugin(adapter, config)
        run = await ctx.subagents.start('any-a2a', request)
      } else {
        run = await adapter.createProvider(config).start(request)
      }
      const result = await run.result
      if (server.fixtureError) throw server.fixtureError
      assert.equal(result.stopReason, scenario === 'input-required' ? 'error' : 'completed')
      if (scenario !== 'input-required') assert.deepEqual(result.output, [{ type: 'text', text: 'answer' }])
      assert.deepEqual(methods, scenario === 'poll' ? ['message/send', 'tasks/get'] : ['message/send'])
    } finally {
      await run?.dispose()
      await ctx?.fiber.dispose()
      if (previous === undefined) delete process.env.ANY_A2A_TOKEN
      else process.env.ANY_A2A_TOKEN = previous
      server.closeAllConnections()
      await new Promise(resolve => server.close(resolve))
    }
  })
}
