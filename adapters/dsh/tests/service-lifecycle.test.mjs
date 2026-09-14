import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { createProvider } from '../src/index.js'

test('remote start publishes before completion and disposal aborts local wait', async () => {
  let entered
  const received = new Promise(resolve => entered = resolve)
  const server = createServer((_req, _res) => entered())
  await new Promise(r => server.listen(0, '127.0.0.1', r))
  let run
  try {
    const provider = createProvider({ serviceUrl: `http://127.0.0.1:${server.address().port}`, agentId: 'test' })
    let timer
    try {
      run = await Promise.race([
        provider.start({ prompt: [{ type: 'text', text: 'test' }], signal: new AbortController().signal }),
        new Promise((_, reject) => { timer = setTimeout(() => reject(Error('start blocked on remote response')), 1000) }),
      ])
    } finally { clearTimeout(timer) }
    await received
    let settled = false
    run.result.then(() => { settled = true })
    await Promise.resolve()
    assert.equal(settled, false)
    await run.dispose()
    assert.equal((await run.result).stopReason, 'aborted')
    await run.dispose()
  } finally {
    await run?.dispose()
    server.closeAllConnections()
    await new Promise(r => server.close(r))
  }
})
