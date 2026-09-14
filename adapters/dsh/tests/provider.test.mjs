import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import * as plugin from '../src/index.js'

const executable = fileURLToPath(new URL('./fixture-cli.mjs', import.meta.url))
const config = { cardUrl: 'https://example.test/card', executable }
const request = (text, signal = new AbortController().signal) => ({ prompt: [{ type: 'text', text }], signal })
async function outcome(text, options = {}) {
  const run = await plugin.createProvider({ ...config, ...options }).start(request(text))
  try { return await run.result } finally { await run.dispose() }
}

test('named loader metadata and truthful capabilities', () => {
  assert.equal(plugin.default, undefined)
  assert.deepEqual(plugin.inject, ['subagents'])
  const provider = plugin.createProvider(config)
  assert.equal(provider.prepareContinuable, undefined)
  assert.equal(provider.inheritsParentContext, false)
  assert.ok(Object.values(provider.capabilities).every(value => value === false))
})

test('argument transport preserves shell metacharacters, Unicode, and text block order', async () => {
  const provider = plugin.createProvider(config)
  const text = 'hello 世界; $(echo unsafe) "quoted"\n--card other'
  const req = request(text)
  req.prompt.push({ type: 'text', text: 'second' })
  const run = await provider.start(req)
  assert.match(run.id, /^any-a2a:/)
  assert.equal(run.localAgent, undefined)
  assert.deepEqual(await run.result, { output: [{ type: 'text', text: `${text}\nsecond` }], stopReason: 'completed' })
  await run.dispose()
  await run.dispose()
})

test('multiple cached cards retain independent names and literal --card-file argv', async () => {
  for (const [providerName, cardFile] of [['card-one', '/tmp/one card;$(touch nope).json'], ['card-two', '/tmp/世界.json']]) {
    const provider = plugin.createProvider({ providerName, cardFile, executable })
    assert.equal(provider.name, providerName)
    const run = await provider.start(request('args'))
    try {
      assert.deepEqual(JSON.parse((await run.result).output[0].text), { cardFlag: '--card-file', card: cardFile })
    } finally { await run.dispose() }
  }
  for (const bad of [{ ...config, cardFile: '/tmp/card.json' }, { cardFile: 'relative.json' }, { cardFile: '/tmp/NUL\0.json' }]) {
    assert.throws(() => plugin.createProvider(bad))
  }
})

test('local service mode delegates through shared transport', async () => {
  const server = (await import('node:http')).createServer((req, res) => {
    let body = ''; req.on('data', chunk => body += chunk); req.on('end', () => {
      assert.equal(req.url, '/api/run'); assert.deepEqual(JSON.parse(body), { id: 'agent-1', message: 'hello' })
      res.setHeader('content-type', 'application/json'); res.end(JSON.stringify({ text: 'answer', state: 'completed' }))
    })
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  try {
    const run = await plugin.createProvider({ serviceUrl: `http://127.0.0.1:${server.address().port}`, agentId: 'agent-1' }).start(request('hello'))
    assert.deepEqual(await run.result, { output: [{ type: 'text', text: 'answer' }], stopReason: 'completed' })
    await run.dispose()
  } finally { server.close() }
})

test('service result preserves structured remote parts instead of text preview', async () => {
  const raw = {kind:'message',parts:[{kind:'data',data:{temperature:23}},{kind:'text',text:'23°C'}]}
  const server = (await import('node:http')).createServer((_req,res) => {
    res.setHeader('content-type','application/json'); res.end(JSON.stringify({raw,text:'23°C',state:'completed'}))
  })
  await new Promise(r=>server.listen(0,'127.0.0.1',r))
  try {
    const run = await plugin.createProvider({serviceUrl:`http://127.0.0.1:${server.address().port}`,agentId:'test'}).start(request('hello'))
    assert.deepEqual(JSON.parse((await run.result).output[0].text),raw)
    await run.dispose()
  } finally { server.closeAllConnections(); await new Promise(r=>server.close(r)) }
})

test('CLI errors and malformed output are sanitized', async () => {
  for (const message of ['nonzero', 'invalid', 'fields', 'overflow']) {
    const result = await outcome(message, { maxOutputBytes: 2000 })
    assert.equal(result.stopReason, 'error')
    assert.deepEqual(result.output, [])
    assert.doesNotMatch(result.diagnostic, /secret-token|private-prompt/)
  }
})

test('remote states never promise continuation or false completion', async () => {
  for (const [state, stopReason] of Object.entries({ completed: 'completed', canceled: 'aborted', rejected: 'refusal', failed: 'error', working: 'error', 'input-required': 'error', 'auth-required': 'error', unknown: 'error' })) {
    assert.equal((await outcome(`state:${state}`)).stopReason, stopReason)
  }
})

test('token forwarded via environment; unrelated ambient secrets scrubbed', async () => {
  const previous = { token: process.env.ANY_A2A_TOKEN, secret: process.env.TEST_SECRET }
  process.env.ANY_A2A_TOKEN = 'test-token'
  process.env.TEST_SECRET = 'never-forward'
  try {
    assert.deepEqual(JSON.parse((await outcome('env')).output[0].text), { token: 'test-token' })
  } finally {
    for (const [key, value] of [['ANY_A2A_TOKEN', previous.token], ['TEST_SECRET', previous.secret]]) {
      if (value === undefined) delete process.env[key]
      else process.env[key] = value
    }
  }
})

test('invalid config, unsupported input, missing executable, and pre-abort fail before publication', async () => {
  for (const bad of [{}, { cardUrl: 'file:///tmp/card' }, { cardUrl: 'https://user:pass@example.test' }, { ...config, maxOutputBytes: 0 }, { ...config, typo: true }]) {
    assert.throws(() => plugin.createProvider(bad))
  }
  const provider = plugin.createProvider(config)
  await assert.rejects(provider.start({ ...request('hello'), prompt: [{ type: 'image' }] }), /only text/)
  await assert.rejects(provider.start({ ...request('hello'), persona: 'x' }), /unsupported/)
  await assert.rejects(provider.start(request('NUL\0')), /NUL/)
  await assert.rejects(provider.start(request('hello', AbortSignal.abort())), { name: 'AbortError' })
  await assert.rejects(plugin.createProvider({ ...config, executable: '/nonexistent/any-a2a' }).start(request('hello')), /could not spawn/)
})

for (const mode of ['abort', 'dispose']) {
  test(`${mode} kills the child and waits for process exit; sibling remains independent`, async () => {
    const directory = await mkdtemp(join(tmpdir(), 'any-a2a-dsh-'))
    const pidFile = join(directory, 'pid')
    const controller = new AbortController()
    const provider = plugin.createProvider(config)
    const run = await provider.start(request(`wait:${pidFile}`, controller.signal))
    try {
      let pid
      for (let attempt = 0; attempt < 200; attempt++) {
        try { pid = Number(await readFile(pidFile, 'utf8')); break } catch (error) { if (error.code !== 'ENOENT') throw error }
        await new Promise(resolve => setTimeout(resolve, 10))
      }
      assert.ok(pid, 'child became ready')
      const sibling = await provider.start(request('hello'))
      if (mode === 'abort') controller.abort()
      await run.dispose()
      assert.equal((await run.result).stopReason, 'aborted')
      assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' })
      assert.equal((await sibling.result).stopReason, 'completed')
      assert.notEqual(run.id, sibling.id)
      await sibling.dispose()
    } finally {
      await run.dispose()
      await rm(directory, { recursive: true, force: true })
    }
  })
}
