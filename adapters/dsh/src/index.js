/** One-shot DSH subagent provider backed by the any-a2a CLI. */
import { spawn } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { isAbsolute } from 'node:path'
import { runService } from '../../common/src/service-client.js'
import { mountCapabilities } from './capabilities.js'

export const name = 'subagent-any-a2a'
export const inject = ['subagents']

/** Validate deployment configuration before registering the provider. */
function resolveConfig(config) {
  if (!config || typeof config !== 'object') throw new Error('any-a2a: configuration is required')
  const allowed = new Set(['cardUrl', 'cardFile', 'serviceUrl', 'serviceToken', 'agentId', 'toolName', 'providerName', 'executable', 'maxOutputBytes'])
  for (const key of Object.keys(config)) {
    if (!allowed.has(key)) throw new Error('any-a2a: unknown configuration field')
  }
  const resolved = {
    cardUrl: config.cardUrl,
    serviceUrl: config.serviceUrl,
    serviceToken: config.serviceToken,
    agentId: config.agentId,
    toolName: config.toolName,
    cardFile: config.cardFile,
    providerName: config.providerName ?? 'any-a2a',
    executable: config.executable ?? 'any-a2a',
    maxOutputBytes: config.maxOutputBytes ?? 1024 * 1024,
  }
  if (resolved.serviceUrl !== undefined) {
    if (resolved.cardUrl !== undefined || resolved.cardFile !== undefined) throw new Error('any-a2a: serviceUrl is mutually exclusive with cardUrl/cardFile')
    if (typeof resolved.serviceUrl !== 'string' || !resolved.serviceUrl.trim()) throw new Error('any-a2a: serviceUrl must be non-empty')
    if (typeof resolved.agentId !== 'string' || !resolved.agentId.trim()) throw new Error('any-a2a: agentId must be non-empty')
  } else if ((resolved.cardUrl !== undefined) === (resolved.cardFile !== undefined)) {
    throw new Error('any-a2a: specify exactly one of cardUrl or cardFile')
  }
  for (const key of [resolved.serviceUrl !== undefined ? 'serviceUrl' : (resolved.cardFile !== undefined ? 'cardFile' : 'cardUrl'), 'providerName', 'executable']) {
    if (typeof resolved[key] !== 'string' || !resolved[key].trim() || resolved[key].includes('\0')) {
      throw new Error(`any-a2a: ${key} must be a non-empty string without NUL`)
    }
  }
  if (resolved.cardFile !== undefined) {
    if (!isAbsolute(resolved.cardFile)) throw new Error('any-a2a: cardFile must be an absolute path')
  } else if (resolved.serviceUrl === undefined) {
    let url
    try { url = new URL(resolved.cardUrl) } catch { throw new Error('any-a2a: cardUrl must be an HTTP(S) URL') }
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) {
      throw new Error('any-a2a: cardUrl must be an HTTP(S) URL without userinfo')
    }
  }
  if (!Number.isSafeInteger(resolved.maxOutputBytes) || resolved.maxOutputBytes < 1) {
    throw new Error('any-a2a: maxOutputBytes must be a positive safe integer')
  }
  return Object.freeze(resolved)
}

/** Only the CLI's designated token is forwarded among ambient credentials. */
function childEnvironment() {
  return Object.fromEntries(Object.entries(process.env).filter(([key]) =>
    key === 'ANY_A2A_TOKEN' || !/KEY|SECRET|TOKEN|PASSWORD/i.test(key)))
}

function failure(diagnostic) {
  return { output: [], stopReason: 'error', diagnostic }
}

function decode(stdout) {
  let value
  try { value = JSON.parse(stdout) } catch { return failure('any-a2a: invalid CLI JSON response') }
  if (!value || typeof value !== 'object' || Array.isArray(value)
    || typeof value.text !== 'string'
    || !['task_id', 'context_id'].every(key => value[key] === null || typeof value[key] === 'string')
    || typeof value.state !== 'string') {
    return failure('any-a2a: invalid CLI response fields')
  }
  const output = value.text ? [{ type: 'text', text: value.text }] : []
  switch (value.state) {
    case 'completed': return { output, stopReason: 'completed' }
    case 'canceled': return { output, stopReason: 'aborted' }
    case 'rejected': return { output, stopReason: 'refusal' }
    case 'failed': return { output, stopReason: 'error', diagnostic: 'any-a2a: remote task failed' }
    default: return { output, stopReason: 'error', diagnostic: 'any-a2a: remote task did not complete; continuation is unsupported' }
  }
}

/**
 * Create an isolated one-shot provider. No parent context or continuation is supported.
 * @param {import('./index.js').Config} config Deployment configuration.
 * @returns {import('@deepseek-ai/dsh-subagent').SubagentProvider} DSH provider.
 */
export function createProvider(config) {
  const resolved = resolveConfig(config)
  return {
    name: resolved.providerName,
    capabilities: { agentOptions: false, outputSchema: false, depthLimit: false, toolFilter: false, persona: false },
    inheritsParentContext: false,
    async start(request) {
      request.signal.throwIfAborted()
      if (['agentOptions', 'outputSchema', 'maxDepth', 'toolFilter', 'persona'].some(key => request[key] !== undefined)) {
        throw new Error('any-a2a: unsupported start option')
      }
      if (request.prompt.some(block => block.type !== 'text')) {
        throw new Error('any-a2a: only text prompt blocks are supported')
      }
      const message = request.prompt.map(block => block.text).join('\n')
      if (message.includes('\0')) throw new Error('any-a2a: prompt contains NUL')
      if (resolved.serviceUrl !== undefined) {
        const controller = new AbortController()
        const abort = () => controller.abort()
        request.signal.addEventListener('abort', abort, { once: true })
        if (request.signal.aborted) abort()
        const result = runService({ serviceUrl: resolved.serviceUrl, serviceToken: resolved.serviceToken, agentId: resolved.agentId, message, signal: controller.signal })
          .then(value => ({
            // DSH's text-only remote result cannot carry arbitrary A2A parts natively.
            // Preserve the complete final A2A object as JSON rather than silently flattening it.
            output: value.raw !== undefined
              ? [{ type: 'text', text: JSON.stringify(value.raw) }]
              : value.text ? [{ type: 'text', text: value.text }] : [],
            stopReason: value.state === 'completed' ? 'completed' : value.state === 'canceled' ? 'aborted' : value.state === 'rejected' ? 'refusal' : 'error',
          }))
          .catch(() => controller.signal.aborted
            ? { output: [], stopReason: 'aborted' }
            : failure('any-a2a: remote service request failed'))
          .finally(() => request.signal.removeEventListener('abort', abort))
        return {
          id: `any-a2a:${randomUUID()}`, localAgent: undefined, result,
          async dispose() { abort(); await result },
        }
      }
      const cardArgs = resolved.cardFile !== undefined ? ['--card-file', resolved.cardFile] : ['--card', resolved.cardUrl]
      const child = spawn(resolved.executable, ['run', ...cardArgs, '--message', message], {
        shell: false, stdio: ['ignore', 'pipe', 'pipe'], env: childEnvironment(),
      })
      let aborted = false
      let overflow = false
      let processError = false
      let closed = false
      let bytes = 0
      const chunks = []
      const kill = () => { if (!closed) child.kill('SIGKILL') }
      const abort = () => { aborted = true; kill() }
      child.stdout.on('data', chunk => {
        bytes += chunk.length
        if (bytes > resolved.maxOutputBytes) { overflow = true; kill() }
        else chunks.push(chunk)
      })
      // Drain, but never retain or disclose stderr: it may contain wire payloads or secrets.
      child.stderr.resume()
      child.on('error', () => { processError = true })
      const result = new Promise(resolve => {
        child.once('close', (code, signal) => {
          closed = true
          request.signal.removeEventListener('abort', abort)
          if (aborted) resolve({ output: [], stopReason: 'aborted' })
          else if (overflow) resolve(failure('any-a2a: CLI stdout exceeded maxOutputBytes'))
          else if (processError || code !== 0 || signal !== null) resolve(failure('any-a2a: CLI process failed'))
          else resolve(decode(Buffer.concat(chunks).toString('utf8')))
        })
      })
      request.signal.addEventListener('abort', abort, { once: true })
      if (request.signal.aborted) abort()
      try {
        await new Promise((resolve, reject) => {
          child.once('spawn', resolve)
          child.once('error', () => reject(new Error('any-a2a: could not spawn CLI executable')))
        })
        request.signal.throwIfAborted()
      } catch (error) {
        abort()
        await result
        throw error
      }
      return {
        // Remote DSH identity, deliberately unrelated to the A2A task/context identifiers.
        id: /** @type {import('@deepseek-ai/dsh-session').SessionId} */ (`any-a2a:${randomUUID()}`),
        localAgent: undefined,
        result,
        async dispose() { if (!closed) abort(); await result },
      }
    },
  }
}

/**
 * Register the provider using DSH's effect-scoped registry; no default export.
 * @param {import('@deepseek-ai/cordis').Context} ctx Host context with subagents service.
 * @param {import('./index.js').Config} config Deployment configuration.
 */
export function apply(ctx, config) {
  ctx.subagents.registerProvider(createProvider(config))
  if (config.serviceUrl) mountCapabilities(ctx, config)
}
