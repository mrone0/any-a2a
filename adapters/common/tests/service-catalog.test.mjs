import test from 'node:test'
import assert from 'node:assert/strict'
import { spawn, execFile } from 'node:child_process'
import { createServer, request } from 'node:http'
import { mkdtemp, writeFile, rm, realpath } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const execute = promisify(execFile)
const executable = process.env.ANY_A2A_TEST_BINARY || fileURLToPath(new URL(
  '../../../target/debug/any-a2a' + (process.platform === 'win32' ? '.exe' : ''), import.meta.url,
))
const token = 'isolated-service-fixture-only'

function deferred() {
  let resolve
  const promise = new Promise(done => { resolve = done })
  return { promise, resolve }
}

async function within(promise, milliseconds, label) {
  let timer
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`${label} timed out`)), milliseconds) }),
    ])
  } finally {
    clearTimeout(timer)
  }
}

function gate() {
  const ready = deferred()
  let released = false
  return {
    get released() { return released },
    wait: () => within(ready.promise, 8_000, 'fixture response gate'),
    release() { released = true; ready.resolve() },
  }
}

function card(origin, endpoint = '/rpc', name = 'fixture') {
  return {
    name, description: 'isolated loopback fixture', version: '1', protocolVersion: '0.3.0',
    preferredTransport: 'JSONRPC', url: origin + endpoint, capabilities: {},
    defaultInputModes: ['text/plain'], defaultOutputModes: ['text/plain'], skills: [],
  }
}

async function readJSON(req) {
  const buffers = []
  for await (const bytes of req) buffers.push(bytes)
  return JSON.parse(Buffer.concat(buffers).toString('utf8'))
}

function sendJSON(res, value, status = 200) {
  res.writeHead(status, { 'content-type': 'application/json' })
  res.end(JSON.stringify(value))
}

function rpcReply(res, rpc, result) {
  sendJSON(res, { jsonrpc: '2.0', id: rpc.id, result })
}

async function fixture(handler) {
  const errors = []
  const server = createServer((req, res) => {
    Promise.resolve(handler(req, res)).catch(error => {
      errors.push(error)
      if (!res.headersSent) sendJSON(res, { error: 'isolated fixture failed' }, 500)
      else res.destroy()
    })
  })
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  return {
    origin: `http://127.0.0.1:${server.address().port}`,
    errors,
    async close() {
      const closed = new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()))
      server.closeAllConnections()
      await within(closed, 3_000, 'loopback fixture shutdown')
    },
  }
}

function scopedEnv(directory) {
  return {
    ...process.env,
    ANY_A2A_DATA_DIR: directory,
    ANY_A2A_SERVICE_TOKEN: token,
    ANY_A2A_TOKEN: '',
  }
}

async function service(directory) {
  const child = spawn(executable, ['serve'], {
    env: scopedEnv(directory), windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
  })
  let stderr = ''
  child.stderr.on('data', bytes => { stderr = (stderr + bytes.toString()).slice(-2_048) })
  const closed = new Promise(resolve => child.once('close', resolve))
  const startup = new Promise((resolve, reject) => {
    let stdout = ''
    child.once('error', reject)
    child.once('exit', code => reject(new Error(`native service exited during startup (${code}): ${stderr}`)))
    child.stdout.on('data', bytes => {
      stdout += bytes.toString()
      const line = stdout.split('\n')[0].trim()
      if (/^127\.0\.0\.1:\d+$/.test(line)) resolve(`http://${line}`)
    })
  })
  try {
    return {
      origin: await within(startup, 3_000, 'native service startup'),
      async close() {
        if (child.exitCode === null && child.signalCode === null) child.kill()
        await within(closed, 3_000, 'native service shutdown')
      },
    }
  } catch (error) {
    if (child.exitCode === null && child.signalCode === null) child.kill()
    await within(closed, 3_000, 'failed native service shutdown')
    throw error
  }
}

function api(origin, path, body, timeout = 3_000) {
  return new Promise((resolve, reject) => {
    const encoded = body === undefined ? null : JSON.stringify(body)
    const req = request(origin + path, {
      method: encoded === null ? 'GET' : 'POST',
      headers: {
        authorization: `Bearer ${token}`,
        ...(encoded === null ? {} : { 'content-type': 'application/json', 'content-length': Buffer.byteLength(encoded) }),
      },
    })
    const timer = setTimeout(() => req.destroy(new Error(`${path} timed out`)), timeout)
    req.once('error', error => { clearTimeout(timer); reject(error) })
    req.once('response', res => {
      const buffers = []
      let length = 0
      res.on('data', bytes => {
        length += bytes.length
        if (length > 8 * 1024 * 1024) req.destroy(new Error('isolated service response exceeded test limit'))
        else buffers.push(bytes)
      })
      res.once('error', error => { clearTimeout(timer); reject(error) })
      res.once('end', () => {
        clearTimeout(timer)
        try { resolve({ status: res.statusCode, value: JSON.parse(Buffer.concat(buffers).toString('utf8')) }) }
        catch (error) { reject(error) }
      })
    })
    req.end(encoded)
  })
}

async function catalog(directory) {
  const { stdout } = await execute(executable, ['catalog'], {
    env: scopedEnv(directory), windowsHide: true, timeout: 3_000, maxBuffer: 8 * 1024 * 1024,
  })
  return JSON.parse(stdout)
}

async function writeCatalog(directory, records) {
  await writeFile(join(directory, 'agent-cards.jsonl'), records.map(row => JSON.stringify(row)).join('\n') + '\n')
}

async function removeFixtureDirectory(directory) {
  const target = await realpath(directory)
  const parent = await realpath(tmpdir())
  assert.equal(dirname(target), parent)
  assert.ok(basename(target).startsWith('any-a2a-service-test-'))
  await rm(target, { recursive: true, force: true })
}

async function cleanup(running, remote, directory, pending = []) {
  await Promise.allSettled(pending)
  const failures = []
  for (const close of [() => running?.close(), () => remote.close(), () => removeFixtureDirectory(directory)]) {
    try { await close() } catch (error) { failures.push(error) }
  }
  if (failures.length) throw new AggregateError(failures, 'isolated service test cleanup failed')
}

test('native catalog and service isolate an unavailable legacy URL from a saved manual agent', { timeout: 15_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'any-a2a-service-test-'))
  let running
  let badGets = 0
  let manualSends = 0
  const remote = await fixture(async (req, res) => {
    if (req.url === '/unavailable-card') { badGets++; return sendJSON(res, {}, 503) }
    assert.equal(req.url, '/rpc')
    const rpc = await readJSON(req)
    assert.equal(rpc.method, 'message/send')
    manualSends++
    rpcReply(res, rpc, { kind: 'message', parts: [{ kind: 'text', text: 'manual-fixture-result' }] })
  })
  try {
    await writeCatalog(directory, [
      { id: 'legacy-unavailable', source: 'url', cardUrl: remote.origin + '/unavailable-card' },
      { id: 'manual', source: 'manual', agentCard: card(remote.origin) },
    ])
    const cliRows = await catalog(directory)
    assert.equal(cliRows.length, 2)
    assert.equal(cliRows.find(row => row.id === 'legacy-unavailable').info, null)
    running = await service(directory)
    const listed = await api(running.origin, '/api/cards')
    assert.equal(listed.status, 200)
    assert.equal(listed.value.find(row => row.id === 'legacy-unavailable').info, null)
    const result = await api(running.origin, '/api/run', { id: 'manual', runId: 'isolated-manual-run', message: 'fixture' })
    assert.equal(result.status, 200)
    assert.equal(result.value.text, 'manual-fixture-result')
    assert.equal(manualSends, 1)
    assert.equal(badGets, 0, 'listing and unrelated execution must not fetch the unavailable card')
    assert.deepEqual(remote.errors, [])
  } finally {
    await cleanup(running, remote, directory)
  }
})

test('native URL execution fetches one current card and never submits to the cached endpoint', { timeout: 15_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'any-a2a-service-test-'))
  let running
  let cardGets = 0
  let freshSends = 0
  let staleSends = 0
  let remote
  remote = await fixture(async (req, res) => {
    if (req.url === '/card') { cardGets++; return sendJSON(res, card(remote.origin, '/fresh-rpc', 'fresh metadata')) }
    const rpc = await readJSON(req)
    assert.equal(rpc.method, 'message/send')
    if (req.url === '/fresh-rpc') freshSends++
    else { assert.equal(req.url, '/stale-rpc'); staleSends++ }
    rpcReply(res, rpc, { kind: 'message', parts: [{ kind: 'text', text: req.url }] })
  })
  try {
    await writeCatalog(directory, [{
      id: 'saved-url', source: 'url', cardUrl: remote.origin + '/card',
      agentCard: card(remote.origin, '/stale-rpc', 'cached metadata'),
    }])
    running = await service(directory)
    const listed = await api(running.origin, '/api/cards')
    assert.equal(listed.status, 200)
    assert.equal(listed.value[0].info.name, 'cached metadata')
    assert.equal(cardGets, 0)
    const result = await api(running.origin, '/api/run', { id: 'saved-url', runId: 'fresh-service-run', message: 'fixture' })
    assert.equal(result.status, 200)
    assert.equal(result.value.text, '/fresh-rpc')
    assert.equal(cardGets, 1)
    const native = await execute(executable, ['run', '--agent-id', 'saved-url', '--message', 'fixture'], {
      env: scopedEnv(directory), windowsHide: true, timeout: 3_000, maxBuffer: 8 * 1024 * 1024,
    })
    assert.equal(JSON.parse(native.stdout).text, '/fresh-rpc')
    assert.equal(cardGets, 2, 'each service/CLI execution should fetch exactly one fresh card')
    assert.equal(freshSends, 2)
    assert.equal(staleSends, 0)
    assert.deepEqual(remote.errors, [])
  } finally {
    await cleanup(running, remote, directory)
  }
})

test('cancellation stays reachable with all eight ordinary service workers waiting on remote HTTP', { timeout: 20_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'any-a2a-service-test-'))
  const sendGate = gate()
  const cardGate = gate()
  const sendSeen = deferred()
  const allCardsSeen = deferred()
  const pending = []
  const track = promise => { promise.catch(() => {}); pending.push(promise); return promise }
  let running
  let cardGets = 0
  let sendCount = 0
  let cancelCount = 0
  let remote
  remote = await fixture(async (req, res) => {
    if (req.url === '/slow-card') {
      cardGets++
      if (cardGets === 7) allCardsSeen.resolve()
      await cardGate.wait()
      return sendJSON(res, card(remote.origin))
    }
    assert.equal(req.url, '/rpc')
    const rpc = await readJSON(req)
    if (rpc.method === 'message/send') {
      sendCount++
      sendSeen.resolve()
      await sendGate.wait()
    } else if (rpc.method === 'tasks/cancel') cancelCount++
    else assert.equal(rpc.method, 'tasks/get')
    rpcReply(res, rpc, {
      kind: 'task', id: 'fixture-task', status: { state: rpc.method === 'tasks/cancel' ? 'canceled' : 'working' },
    })
  })
  try {
    await writeCatalog(directory, [{ id: 'manual', source: 'manual', agentCard: card(remote.origin) }])
    running = await service(directory)
    const live = track(api(running.origin, '/api/run', { id: 'manual', runId: 'saturated-run', message: 'fixture' }, 6_000))
    await within(sendSeen.promise, 2_000, 'manual task submission')
    const imports = Array.from({ length: 7 }, () => track(api(running.origin, '/api/cards', { cardUrl: remote.origin + '/slow-card' }, 6_000)))
    await within(allCardsSeen.promise, 2_000, 'all seven concurrent card imports')
    assert.equal(sendGate.released, false)
    assert.equal(cardGate.released, false)
    const canceled = await api(running.origin, '/api/cancel', { runId: 'saturated-run' }, 1_000)
    assert.equal(canceled.status, 200)
    assert.equal(canceled.value.requested, true)
    assert.equal(sendGate.released, false, 'cancel handling must precede release of the in-flight send')
    assert.equal(cardGate.released, false, 'cancel handling must precede release of card imports')
    sendGate.release()
    const result = await within(live, 2_000, 'confirmed remote cancellation')
    assert.equal(result.status, 200)
    assert.equal(result.value.state, 'canceled')
    assert.equal(cancelCount, 1)
    assert.equal(sendCount, 1)
    assert.equal(cardGate.released, false)
    cardGate.release()
    const saved = await Promise.all(imports)
    assert.ok(saved.every(response => response.status === 200))
    assert.deepEqual(remote.errors, [])
  } finally {
    sendGate.release()
    cardGate.release()
    await cleanup(running, remote, directory, pending)
  }
})

for (const cardStatus of [200, 500]) {
  test(`URL preconnection cancellation registers immediately and prevents submission after Card HTTP ${cardStatus}`, { timeout: 15_000 }, async () => {
    const directory = await mkdtemp(join(tmpdir(), 'any-a2a-service-test-'))
    const cardGate = gate()
    const cardSeen = deferred()
    const pending = []
    let running
    let cardGets = 0
    let sends = 0
    let remote
    remote = await fixture(async (req, res) => {
      if (req.url === '/card') {
        cardGets++
        cardSeen.resolve()
        await cardGate.wait()
        return sendJSON(res, cardStatus === 200 ? card(remote.origin) : {}, cardStatus)
      }
      assert.equal(req.url, '/rpc')
      const rpc = await readJSON(req)
      assert.equal(rpc.method, 'message/send')
      sends++
      rpcReply(res, rpc, { kind: 'message', parts: [{ kind: 'text', text: 'fixture-completed' }] })
    })
    try {
      await writeCatalog(directory, [{ id: 'url', source: 'url', cardUrl: remote.origin + '/card' }])
      running = await service(directory)
      const body = { id: 'url', runId: 'preconnection-run', message: 'fixture' }
      const live = api(running.origin, '/api/run', body, 5_000)
      live.catch(() => {})
      pending.push(live)
      await within(cardSeen.promise, 2_000, 'preconnection Card GET')
      const duplicate = await api(running.origin, '/api/run', body, 1_000)
      assert.equal(duplicate.status, 400)
      assert.match(duplicate.value.error, /Duplicate runId/)
      assert.equal(cardGets, 1, 'duplicate registration must fail before a second Card GET')
      const canceled = await api(running.origin, '/api/cancel', { runId: body.runId }, 1_000)
      assert.equal(canceled.status, 200)
      assert.equal(canceled.value.requested, true)
      assert.equal(cardGate.released, false)
      assert.equal(sends, 0)
      cardGate.release()
      const result = await live
      assert.equal(result.status, 400)
      assert.match(result.value.error, /发送前已停止.*未提交远程任务/)
      assert.equal(cardGets, 1)
      assert.equal(sends, 0, 'a locally stopped preconnection must never call SendMessage')
      const finished = await api(running.origin, '/api/cancel', { runId: body.runId })
      assert.equal(finished.status, 400)
      assert.match(finished.value.error, /运行不存在或已结束/)
      assert.deepEqual(remote.errors, [])
    } finally {
      cardGate.release()
      await cleanup(running, remote, directory, pending)
    }
  })
}

test('failed Card connection releases its run ID before a successful manual reuse', { timeout: 15_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'any-a2a-service-test-'))
  let running
  let cardGets = 0
  let sends = 0
  const remote = await fixture(async (req, res) => {
    if (req.url === '/unavailable-card') { cardGets++; return sendJSON(res, {}, 500) }
    assert.equal(req.url, '/rpc')
    const rpc = await readJSON(req)
    assert.equal(rpc.method, 'message/send')
    sends++
    rpcReply(res, rpc, { kind: 'message', parts: [{ kind: 'text', text: 'reused-fixture-ID' }] })
  })
  try {
    await writeCatalog(directory, [
      { id: 'url', source: 'url', cardUrl: remote.origin + '/unavailable-card' },
      { id: 'manual', source: 'manual', agentCard: card(remote.origin) },
    ])
    running = await service(directory)
    const runId = 'reusable-fixture-run'
    const failed = await api(running.origin, '/api/run', { id: 'url', runId, message: 'fixture' })
    assert.equal(failed.status, 400)
    assert.match(failed.value.error, /HTTP error 500/)
    assert.equal(cardGets, 1)
    assert.equal(sends, 0)
    const ended = await api(running.origin, '/api/cancel', { runId })
    assert.equal(ended.status, 400)
    assert.match(ended.value.error, /运行不存在或已结束/)
    const reused = await api(running.origin, '/api/run', { id: 'manual', runId, message: 'fixture' })
    assert.equal(reused.status, 200)
    assert.equal(reused.value.text, 'reused-fixture-ID')
    assert.equal(sends, 1)
    const completed = await api(running.origin, '/api/cancel', { runId })
    assert.equal(completed.status, 400)
    assert.match(completed.value.error, /运行不存在或已结束/)
    assert.deepEqual(remote.errors, [])
  } finally {
    await cleanup(running, remote, directory)
  }
})

test('invalid run IDs fail before any remote Card request', { timeout: 15_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'any-a2a-service-test-'))
  let running
  let calls = 0
  let remote
  remote = await fixture(async (_req, res) => { calls++; sendJSON(res, card(remote.origin)) })
  try {
    await writeCatalog(directory, [{ id: 'url', source: 'url', cardUrl: remote.origin + '/card' }])
    running = await service(directory)
    for (const runId of [null, 42, '', '   ', 'control\ncharacter', 'x'.repeat(257)]) {
      const result = await api(running.origin, '/api/run', { id: 'url', runId, message: 'fixture' })
      assert.equal(result.status, 400)
      assert.match(result.value.error, /runId/)
    }
    assert.equal(calls, 0)
    assert.deepEqual(remote.errors, [])
  } finally {
    await cleanup(running, remote, directory)
  }
})
