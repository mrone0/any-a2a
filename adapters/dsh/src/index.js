/** One-shot DSH subagent provider backed by the any-a2a CLI. */
import { launchCli, CLI_LIMITS } from '../../common/src/cli-transport.js'
import { randomUUID } from 'node:crypto'
import { isAbsolute } from 'node:path'
import { runService } from '../../common/src/service-client.js'
import { reportProgress } from './progress.js'
import { mountCapabilities } from './capabilities.js'

export const name = 'subagent-any-a2a'
export const inject = ['subagents']

/** Validate deployment configuration before registering the provider. */
function resolveConfig(config) {
  if (!config || typeof config !== 'object') throw new Error('any-a2a: configuration is required')
  const allowed = new Set(['cardUrl', 'cardFile', 'serviceUrl', 'serviceToken', 'agentId', 'toolName', 'providerName', 'executable', 'dataDir', 'maxOutputBytes', 'args'])
  for (const key of Object.keys(config)) {
    if (!allowed.has(key)) throw new Error('any-a2a: unknown configuration field')
  }
  const resolved = {
    dataDir: config.dataDir,
    cardUrl: config.cardUrl,
    serviceUrl: config.serviceUrl,
    serviceToken: config.serviceToken,
    agentId: config.agentId,
    toolName: config.toolName,
    cardFile: config.cardFile,
    providerName: config.providerName ?? 'any-a2a',
    executable: config.executable ?? 'any-a2a',
    maxOutputBytes: config.maxOutputBytes ?? CLI_LIMITS.outputBytes,
    args: config.args ?? [],
  }
  if (resolved.serviceUrl !== undefined) {
    if (resolved.cardUrl !== undefined || resolved.cardFile !== undefined) throw new Error('any-a2a: serviceUrl is mutually exclusive with cardUrl/cardFile')
    if (typeof resolved.serviceUrl !== 'string' || !resolved.serviceUrl.trim()) throw new Error('any-a2a: serviceUrl must be non-empty')
    if (typeof resolved.agentId !== 'string' || !resolved.agentId.trim()) throw new Error('any-a2a: agentId must be non-empty')
  } else if (resolved.agentId !== undefined) {
    if (typeof resolved.agentId !== 'string' || !resolved.agentId.trim() || resolved.agentId.includes('\0')) throw new Error('any-a2a: invalid agentId')
    if (resolved.cardUrl !== undefined || resolved.cardFile !== undefined) throw new Error('any-a2a: agentId is mutually exclusive with cardUrl/cardFile')
  } else if ((resolved.cardUrl !== undefined) === (resolved.cardFile !== undefined)) {
    throw new Error('any-a2a: specify exactly one of cardUrl or cardFile')
  }
  if (resolved.dataDir !== undefined && (typeof resolved.dataDir !== 'string' || !isAbsolute(resolved.dataDir) || resolved.dataDir.includes('\0'))) throw new Error('any-a2a: dataDir must be an absolute path')
  for (const key of [resolved.serviceUrl !== undefined ? 'serviceUrl' : resolved.agentId !== undefined ? 'agentId' : (resolved.cardFile !== undefined ? 'cardFile' : 'cardUrl'), 'providerName', 'executable']) {
    if (typeof resolved[key] !== 'string' || !resolved[key].trim() || resolved[key].includes('\0')) {
      throw new Error(`any-a2a: ${key} must be a non-empty string without NUL`)
    }
  }
  if (resolved.cardFile !== undefined) {
    if (!isAbsolute(resolved.cardFile)) throw new Error('any-a2a: cardFile must be an absolute path')
  } else if (resolved.serviceUrl === undefined && resolved.agentId === undefined) {
    let url
    try { url = new URL(resolved.cardUrl) } catch { throw new Error('any-a2a: cardUrl must be an HTTP(S) URL') }
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) {
      throw new Error('any-a2a: cardUrl must be an HTTP(S) URL without userinfo')
    }
  }
  if (!Number.isSafeInteger(resolved.maxOutputBytes) || resolved.maxOutputBytes < 1) {
    throw new Error('any-a2a: maxOutputBytes must be a positive safe integer')
  }
  if (!Array.isArray(resolved.args) || resolved.args.length > 32
    || resolved.args.some(value => typeof value !== 'string' || value.includes('\0'))
    || resolved.args.reduce((bytes, value) => bytes + Buffer.byteLength(value), 0) > 32 * 1024) {
    throw new Error('any-a2a: args must be trusted executable prefix strings within 32 arguments / 32 KiB')
  }
  resolved.args = Object.freeze([...resolved.args])
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

function decode(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)
    || typeof value.text !== 'string'
    || !['task_id', 'context_id'].every(key => value[key] === null || typeof value[key] === 'string')
    || typeof value.state !== 'string') {
    return failure('any-a2a: invalid CLI response fields')
  }
  const output = value.raw !== undefined ? [{ type: 'text', text: JSON.stringify(value.raw) }] : value.text ? [{ type: 'text', text: value.text }] : []
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
      const cardArgs = resolved.agentId !== undefined ? ['--agent-id', resolved.agentId] : resolved.cardFile !== undefined ? ['--card-file', resolved.cardFile] : ['--card', resolved.cardUrl]
      const streaming = resolved.agentId !== undefined
      const handle = launchCli(['run', ...cardArgs, ...(streaming ? ['--events'] : []), '--message', message], {
        signal: request.signal,
        maxOutputBytes: resolved.maxOutputBytes,
        onProgress: event => reportProgress(request.signal, event),
        config: { executable: resolved.executable, args: resolved.args,
          env: { ...childEnvironment(), ...(resolved.dataDir ? { ANY_A2A_DATA_DIR: resolved.dataDir } : {}) } },
      })
      // Attach the result consumer before awaiting publication: early failures are never unhandled.
      const result = handle.result.then(decode, error => {
        if (error.kind === 'aborted') return { output: [], stopReason: 'aborted' }
        if (error.kind === 'output_limit') return failure('any-a2a: CLI stdout exceeded maxOutputBytes or frame limit')
        if (error.kind === 'protocol') return failure('any-a2a: invalid CLI response')
        if (error.kind === 'timeout') return failure('any-a2a: CLI deadline exceeded; remote task may still be running')
        return failure('any-a2a: CLI process failed')
      })
      try {
        await handle.spawned
        request.signal.throwIfAborted()
      } catch (error) {
        handle.stop()
        await result
        if (error.kind === 'spawn') throw new Error('any-a2a: could not spawn CLI executable')
        throw error
      }
      return {
        // Remote DSH identity, deliberately unrelated to A2A task/context IDs.
        id: /** @type {import('@deepseek-ai/dsh-session').SessionId} */ (`any-a2a:${randomUUID()}`),
        localAgent: undefined,
        result,
        async dispose() { handle.stop(); await result },
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
  if (config.serviceUrl || config.agentId) mountCapabilities(ctx, config)
}
