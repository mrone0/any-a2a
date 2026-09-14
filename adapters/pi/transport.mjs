import { spawn } from 'node:child_process'
import { isAbsolute } from 'node:path'

export function deployment(env = process.env) {
  const executable = env.ANY_A2A_EXECUTABLE || 'any-a2a'
  const dataDir = env.ANY_A2A_DATA_DIR
  if (executable.includes('\0') || (dataDir && (!isAbsolute(dataDir) || dataDir.includes('\0')))) throw Error('Invalid any-a2a deployment paths')
  const childEnv = Object.fromEntries(Object.entries(env).filter(([key]) => !/KEY|SECRET|TOKEN|PASSWORD/i.test(key)))
  return { executable, env: childEnv }
}

/** No shell, bounded stdout, drained but undisclosed stderr; settle after process close. */
export function executeCli(args, { signal, onProgress, timeoutMs = 125000, config = deployment() } = {}) {
  signal?.throwIfAborted()
  return new Promise((resolve, reject) => {
    let failure, total = 0, pending = '', stdout = '', final, closed = false
    const decoder = new TextDecoder()
    const events = args.includes('--events')
    let child
    try { child = spawn(config.executable, args, {shell:false, windowsHide:true, env:config.env, stdio:['ignore','pipe','pipe']}) }
    catch { reject(Error('Cannot start any-a2a CLI')); return }
    const stop = message => { failure ||= message; if (!closed) child.kill('SIGKILL') }
    const abort = () => stop('Local delegation stopped; remote task may still be running. Remote cancellation was NOT confirmed.')
    const timer = setTimeout(() => stop('CLI deadline exceeded; remote task may still be running. No automatic retry.'), timeoutMs)
    const consume = text => {
      pending += text
      let end
      while ((end = pending.indexOf('\n')) >= 0) {
        const line = pending.slice(0,end); pending = pending.slice(end+1)
        if (!line.trim()) continue
        try {
          const value = JSON.parse(line)
          if (value.event === 'progress' && value.data && final === undefined) onProgress?.(value.data)
          else if (value.event === 'result' && final === undefined) final = value.data
          else stop('Invalid CLI event stream')
        } catch { stop('Invalid CLI event stream or progress handler failed') }
      }
    }
    child.stdout.on('data', chunk => {
      total += chunk.length
      if (total > 8 * 1024 * 1024) { stop('CLI output exceeded 8 MiB; remote task may still be running'); return }
      if (failure) return
      const text = decoder.decode(chunk,{stream:true})
      if (events) consume(text); else stdout += text
    })
    child.stderr.resume()
    child.on('error', () => { failure ||= 'Cannot start any-a2a CLI' })
    child.once('close', code => {
      closed = true; clearTimeout(timer); signal?.removeEventListener('abort',abort)
      if (events) consume(decoder.decode()); else stdout += decoder.decode()
      if (failure) { reject(Error(failure)); return }
      if (code !== 0) { reject(Error('A2A CLI failed; remote state unknown. No automatic retry. Check CLI installation and saved Agent configuration.')); return }
      try {
        if (events) {
          if (pending.trim() || !final || typeof final.text !== 'string' || !['completed','canceled'].includes(final.state)
            || !['task_id','context_id'].every(k=>final[k] === null || typeof final[k] === 'string')) throw Error()
          resolve(final)
        } else resolve(JSON.parse(stdout))
      } catch { reject(Error('Invalid CLI response')) }
    })
    signal?.addEventListener('abort',abort,{once:true})
    if (signal?.aborted) abort()
  })
}

export function progressText(event) {
  // Allowlist fields: never publish HTTP origins, headers, tokens or raw diagnostics.
  if (event.stage === 'remote_status' && typeof event.text === 'string') return `Remote progress (untrusted remote content):\n${event.text.slice(0,8000)}`
  if (event.stage === 'task' && typeof event.taskId === 'string' && typeof event.state === 'string') return `Task ${event.taskId.slice(0,200)}: ${event.state.slice(0,80)}`
  if (event.stage === 'transport_error') return 'A2A transport failed; remote state unknown.'
  if (event.stage === 'request' && ['message/send','SendMessage','tasks/cancel','CancelTask'].includes(event.method)) return `A2A ${event.method}`
  return null
}
