import { spawn } from 'node:child_process'

export const CLI_LIMITS = Object.freeze({
  outputBytes: 20 * 1024 * 1024,
  frameBytes: 16 * 1024 * 1024,
  progressFrameBytes: 16 * 1024,
  timeoutMs: 125000,
})

/** Fixed diagnostic vocabulary only; never attach raw stderr, argv, or spawn errors. */
export class CliTransportError extends Error {
  constructor(kind, message) {
    super(message)
    this.name = 'CliTransportError'
    this.kind = kind
  }
}

const positive = (value, name) => {
  if (!Number.isSafeInteger(value) || value < 1) throw new TypeError(`Invalid CLI ${name}`)
  return value
}
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value)
const stages = new Set(['request', 'response', 'task', 'remote_status', 'transport_error', 'progress_truncated'])
const states = new Set(['submitted', 'working', 'completed', 'failed', 'canceled', 'rejected', 'input-required', 'auth-required'])

export function validateOutcome(value) {
  if (!object(value) || typeof value.text !== 'string' || typeof value.state !== 'string'
    || !['task_id', 'context_id'].every(key => value[key] === null || typeof value[key] === 'string')
    || (value.progress_truncated !== undefined && typeof value.progress_truncated !== 'boolean')) {
    throw new CliTransportError('protocol', 'Invalid CLI response fields')
  }
  return value
}

/**
 * A client-owned direct process. Publication is separate from its final outcome.
 * result settles ONLY after close; stop is idempotent and never confirms remote cancel.
 * config.args is a trusted deployment/test prefix, never a model-controlled argument.
 */
export function launchCli(args, {
  signal, onProgress, timeoutMs = CLI_LIMITS.timeoutMs,
  maxOutputBytes = CLI_LIMITS.outputBytes, maxFrameBytes = CLI_LIMITS.frameBytes,
  maxProgressFrameBytes = CLI_LIMITS.progressFrameBytes,
  config = { executable: 'any-a2a', env: process.env },
} = {}) {
  if (signal?.aborted) throw new CliTransportError('aborted', 'Local delegation stopped before CLI launch; no request submitted by this invocation.')
  positive(timeoutMs, 'deadline')
  positive(maxOutputBytes, 'output limit')
  positive(maxFrameBytes, 'frame limit')
  positive(maxProgressFrameBytes, 'progress frame limit')
  if (!Array.isArray(args) || args.some(value => typeof value !== 'string' || value.includes('\0'))
    || typeof config.executable !== 'string' || !config.executable || config.executable.includes('\0')
    || (config.args !== undefined && (!Array.isArray(config.args) || config.args.length > 32
      || config.args.some(value => typeof value !== 'string' || value.includes('\0'))
      || config.args.reduce((bytes, value) => bytes + Buffer.byteLength(value), 0) > 32 * 1024))) {
    throw new TypeError('Invalid CLI deployment or arguments')
  }
  // Deployment may lower a budget; hard ceilings always remain bounded.
  const outputLimit = Math.min(maxOutputBytes, CLI_LIMITS.outputBytes)
  const frameLimit = Math.min(maxFrameBytes, CLI_LIMITS.frameBytes, outputLimit)
  const progressLimit = Math.min(maxProgressFrameBytes, CLI_LIMITS.progressFrameBytes, frameLimit)
  const events = args.includes('--events')
  const isRun = args[0] === 'run'
  let child
  try {
    child = spawn(config.executable, [...(config.args || []), ...args], {
      shell: false, windowsHide: true, env: config.env, stdio: ['ignore', 'pipe', 'pipe'],
    })
  } catch {
    throw new CliTransportError('spawn', 'Cannot start any-a2a CLI')
  }
  let resolveSpawned, rejectSpawned, resolveResult, rejectResult
  const spawned = new Promise((resolve, reject) => { resolveSpawned = resolve; rejectSpawned = reject })
  const result = new Promise((resolve, reject) => { resolveResult = resolve; rejectResult = reject })
  let closed = false, published = false, failure, total = 0, pendingBytes = 0, final
  let lastStage = 'not observed', lastState = 'unknown', transportFailed = false
  let pending = []
  const decoder = new TextDecoder('utf-8', { fatal: true })
  const kill = () => { if (!closed) { try { child.kill('SIGKILL') } catch {} } }
  const fail = (kind, message) => {
    failure ||= new CliTransportError(kind, message)
    kill()
  }
  const stop = () => {
    if (!closed) fail('aborted', 'Local delegation stopped; remote task may still be running. Remote cancellation was NOT confirmed.')
  }
  const timer = setTimeout(() => fail('timeout', 'CLI deadline exceeded; remote task may still be running. No automatic retry.'), Math.min(timeoutMs, CLI_LIMITS.timeoutMs))
  const parseFrame = buffer => {
    let value
    try {
      const text = decoder.decode(buffer)
      if (!text.trim()) return
      value = JSON.parse(text)
    } catch {
      fail('protocol', 'Invalid CLI JSON or UTF-8 response')
      return
    }
    if (!events) { final = value; return }
    if (!object(value) || final !== undefined) { fail('protocol', 'Invalid CLI event stream'); return }
    if (value.event === 'result') {
      try { final = validateOutcome(value.data) }
      catch { fail('protocol', 'Invalid CLI response fields') }
      return
    }
    if (value.event !== 'progress' || !object(value.data) || !stages.has(value.data.stage)) {
      fail('protocol', 'Invalid CLI event stream')
      return
    }
    if (buffer.length > progressLimit) { fail('output_limit', 'CLI progress frame exceeded its byte limit; remote task may still be running'); return }
    const event = value.data
    lastStage = event.stage
    if (states.has(event.state)) lastState = event.state
    if (event.stage === 'transport_error') transportFailed = true
    try { onProgress?.(event) }
    catch { fail('protocol', 'CLI progress handler failed') }
  }
  const append = chunk => {
    pendingBytes += chunk.length
    if (pendingBytes > frameLimit) {
      fail('output_limit', 'CLI frame exceeded its byte limit; remote task may still be running')
      return false
    }
    if (chunk.length) pending.push(chunk)
    return true
  }
  child.stdout.on('data', chunk => {
    total += chunk.length
    if (total > outputLimit) { fail('output_limit', 'CLI stdout exceeded maxOutputBytes; remote task may still be running'); return }
    if (failure) return
    if (!events) { append(chunk); return }
    let offset = 0, newline
    while ((newline = chunk.indexOf(10, offset)) !== -1) {
      if (!append(chunk.subarray(offset, newline))) return
      const frame = Buffer.concat(pending, pendingBytes)
      pending = []; pendingBytes = 0
      parseFrame(frame)
      if (failure) return
      offset = newline + 1
    }
    append(chunk.subarray(offset))
  })
  child.stderr.resume()
  child.once('spawn', () => { published = true; resolveSpawned() })
  child.on('error', error => {
    const code = typeof error.code === 'string' && /^[A-Z0-9_]{1,30}$/.test(error.code) ? ` (${error.code})` : ''
    const safe = new CliTransportError('spawn', `Cannot start any-a2a CLI${code}`)
    failure ||= safe
    if (!published) rejectSpawned(safe)
    kill()
  })
  child.once('close', (code, exitSignal) => {
    closed = true
    clearTimeout(timer)
    signal?.removeEventListener('abort', stop)
    if (!published) rejectSpawned(failure || new CliTransportError('spawn', 'Cannot start any-a2a CLI'))
    if (!failure && (code !== 0 || exitSignal !== null)) {
      failure = new CliTransportError('exit', `A2A CLI exited with code ${code}; last observed stage: ${lastStage}; remote state: ${lastState}${transportFailed ? '; transport failure observed' : ''}. No automatic retry.`)
    }
    if (!failure) {
      if (events && pendingBytes) failure = new CliTransportError('protocol', 'Invalid CLI event stream: incomplete final frame')
      else if (!events) parseFrame(Buffer.concat(pending, pendingBytes))
      if (!failure && final === undefined) failure = new CliTransportError('protocol', 'Invalid CLI response: no final result')
      if (!failure && isRun) {
        try { validateOutcome(final) } catch (error) { failure = error }
      }
    }
    pending = []; pendingBytes = 0
    if (failure) rejectResult(failure)
    else resolveResult(final)
  })
  signal?.addEventListener('abort', stop, { once: true })
  if (signal?.aborted) stop()
  return { spawned, result, stop }
}

export function executeCli(args, options) {
  const handle = launchCli(args, options)
  handle.spawned.catch(() => {})
  return handle.result
}
