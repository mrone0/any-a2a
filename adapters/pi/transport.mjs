import { executeCli as executeCommonCli, CliTransportError } from '../common/src/cli-transport.js'
import { isAbsolute } from 'node:path'
import { fileURLToPath } from 'node:url'
import { existsSync } from 'node:fs'

// pi install of a local package retains its source path. Resolve from this
// module, never the caller's cwd; explicit deployment settings take priority.
export function localExecutable(platform = process.platform) {
  const suffix = platform === 'win32' ? '.exe' : ''
  return [`./bin/any-a2a${suffix}`, ...['release', 'debug'].map(profile => `../../target/${profile}/any-a2a${suffix}`)]
    .map(relative => fileURLToPath(new URL(relative, import.meta.url)))
    .find(path => existsSync(path))
}

export function deployment(env = process.env) {
  const executable = env.ANY_A2A_EXECUTABLE || localExecutable() || 'any-a2a'
  const dataDir = env.ANY_A2A_DATA_DIR
  if (executable.includes('\0') || (dataDir && (!isAbsolute(dataDir) || dataDir.includes('\0')))) throw Error('Invalid any-a2a deployment paths')
  const childEnv = Object.fromEntries(Object.entries(env).filter(([key]) => !/KEY|SECRET|TOKEN|PASSWORD/i.test(key)))
  return { executable, env: childEnv }
}

/** Shared bounded process transport; this adapter only presents supported terminal results. */
export function executeCli(args, options = {}) {
  const pending = executeCommonCli(args, {...options, config:options.config || deployment()})
  return pending.then(value => {
    if (args[0] === 'run' && !['completed','canceled'].includes(value.state)) {
      throw new CliTransportError('protocol', 'Remote task did not complete; continuation is unsupported')
    }
    return value
  })
}

export function progressText(event) {
  // Allowlist fields: never publish HTTP origins, headers, tokens or raw diagnostics.
  if (event.stage === 'remote_status' && typeof event.text === 'string') return `Remote progress (untrusted remote content):\n${event.text.slice(0,8000)}`
  if (event.stage === 'task' && typeof event.taskId === 'string' && typeof event.state === 'string') return `Task ${event.taskId.slice(0,200)}: ${event.state.slice(0,80)}`
  if (event.stage === 'progress_truncated') return 'A2A progress was truncated by retention limits; final result collection continues.'
  if (event.stage === 'transport_error') return 'A2A transport failed; remote state unknown.'
  if (event.stage === 'request' && ['message/send','SendMessage','tasks/cancel','CancelTask'].includes(event.method)) return `A2A ${event.method}`
  return null
}
