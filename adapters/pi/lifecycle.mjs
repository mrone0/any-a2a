import { randomUUID } from 'node:crypto'

export class RunPersistenceError extends Error {
  constructor(phase, record) {
    super(phase === 'initial'
      ? 'Could not save the initial delegation record; no remote task was submitted.'
      : 'Could not save the terminal delegation record; saved state may be stale. Do not resubmit the task.')
    this.name = 'RunPersistenceError'
    this.phase = phase
    this.record = record
  }
}

/** Pi-independent admission, cancellation and terminal persistence for one extension instance. */
export class RunLifecycle {
  constructor({maxActive = 4, maxRecords = 200, now = Date.now, createId = randomUUID} = {}) {
    this.maxActive = maxActive
    this.maxRecords = maxRecords
    this.now = now
    this.createId = createId
    this.entries = new Map()
    this.ownerLocks = new Map()
    this.closed = false
    this.shuttingDown = false
    this.shutdownPromise = null
  }

  get size() { return this.entries.size }

  activate() {
    if (this.shuttingDown || this.entries.size) throw Error('Previous A2A lifecycle has not finished shutting down')
    this.closed = false
    this.shutdownPromise = null
  }

  get(id, owner) {
    const entry = this.entries.get(id)
    if (entry && entry.owner !== owner) throw Error('A2A run owner mismatch')
    return entry
  }

  async stop(id, owner) {
    const entry = this.get(id, owner)
    if (!entry) return false
    entry.controller.abort()
    await entry.done
    return true
  }

  shutdown() {
    if (this.shutdownPromise) return this.shutdownPromise
    this.closed = true
    this.shuttingDown = true
    const entries = [...this.entries.values()]
    for (const entry of entries) entry.controller.abort()
    this.shutdownPromise = Promise.allSettled(entries.map(entry => entry.done))
      .then(() => undefined).finally(() => { this.shuttingDown = false })
    return this.shutdownPromise
  }

  async withOwner(owner, operation) {
    const previous = this.ownerLocks.get(owner) || Promise.resolve()
    let release
    const gate = new Promise(resolve => { release = resolve })
    const tail = previous.then(() => gate)
    this.ownerLocks.set(owner, tail)
    await previous
    try { return await operation() }
    finally {
      release()
      if (this.ownerLocks.get(owner) === tail) this.ownerLocks.delete(owner)
    }
  }

  reserve({owner, signal, background, record, store, onTerminal}) {
    if (!owner || typeof owner !== 'string') throw Error('Parent session identity required')
    if (this.closed) throw Error('A2A session is shutting down; no task submitted')
    if (signal?.aborted) throw Error('Local delegation stopped before submission; no remote task was submitted.')
    if (this.entries.size >= this.maxActive) throw Error(`At most ${this.maxActive} live A2A runs`)
    const id = this.createId(), controller = new AbortController(), startedAt = this.now()
    let finish
    const done = new Promise(resolve => { finish = resolve })
    const entry = {
      id, owner, controller, done, result: null, store, onTerminal,
      background: !!background, claimed: false, saved: false, launched: false,
      record: {...record, version:1, id, owner, background:!!background,
        state:'running', startedAt, updatedAt:startedAt, progress:[]},
    }
    const abort = () => controller.abort()
    let detached = false, released = false
    entry.detachCaller = () => {
      if (detached) return
      detached = true
      signal?.removeEventListener('abort', abort)
    }
    entry.publish = () => {
      if (controller.signal.aborted) throw Error(this.stoppedMessage(entry))
      if (background) entry.detachCaller()
    }
    entry.release = () => {
      if (released) return
      released = true
      entry.detachCaller()
      entry.claimed = false
      this.entries.delete(id)
      finish()
    }
    // Registration and signal linking happen synchronously, before the first await.
    this.entries.set(id, entry)
    signal?.addEventListener('abort', abort, {once:true})
    if (signal?.aborted) abort()
    return entry
  }

  async saveTerminal(entry) {
    try { await entry.store.save(entry.record) }
    catch { throw new RunPersistenceError('terminal', entry.record) }
    // Exactly one terminal callback per invocation. Delivery failures are not retried.
    await entry.onTerminal?.(entry.record)
  }

  stoppedMessage(entry) {
    return entry.launched
      ? 'Local delegation stopped; remote state unknown. Remote cancellation was not confirmed.'
      : 'Local delegation stopped before submission; no remote task was submitted.'
  }

  async fail(entry, error) {
    if (entry.record.state !== 'running') return
    const endedAt = this.now()
    Object.assign(entry.record, {
      state:entry.controller.signal.aborted ? 'stopped' : 'failed', endedAt, updatedAt:endedAt,
      error:entry.controller.signal.aborted
        ? this.stoppedMessage(entry)
        : String(error?.message || 'Local delegation failed').slice(0,8000),
    })
    // A failed initial save does not establish a durable run; do not claim or retry it.
    if (entry.saved) await this.saveTerminal(entry)
  }

  async collect(entry, launched) {
    try {
      const outcome = await launched
      if (!outcome || !['completed','canceled'].includes(outcome.state)) throw Error('Remote task did not complete; continuation is unsupported')
      const endedAt = this.now()
      Object.assign(entry.record, {
        state:outcome.state === 'canceled' ? 'stopped' : 'completed', remoteState:outcome.state,
        endedAt, updatedAt:endedAt, outcome,
        progressTruncated:!!entry.record.progressTruncated || outcome.progress_truncated === true,
      })
      await this.saveTerminal(entry)
      return outcome
    } catch (error) {
      await this.fail(entry,error)
      if (!(error instanceof RunPersistenceError) && entry.record.state === 'stopped') throw Error(entry.record.error)
      throw error
    } finally { entry.release() }
  }

  /**
   * Resolves once preflight, initial snapshot and local launch are accepted.
   * The returned result settles after terminal persistence; done always settles after cleanup.
   * preflight/launch/onInitial are trusted adapter callbacks, never model-supplied code.
   */
  async start({owner, signal, background = false, record, store, countRecords, preflight, launch, onInitial, onTerminal}) {
    const entry = this.reserve({owner, signal, background, record, store, onTerminal})
    try {
      await this.withOwner(owner,async() => {
        entry.controller.signal.throwIfAborted()
        const count = await (countRecords ? countRecords() : typeof store.count === 'function' ? store.count() : store.list().then(rows => rows.length))
        entry.controller.signal.throwIfAborted()
        const pending = [...this.entries.values()].filter(other => other.owner === owner && other.claimed).length
        if (!Number.isSafeInteger(count) || count < 0 || count + pending >= this.maxRecords) throw Error('Run retention limit: archive older records first')
        entry.claimed = true
      })
      entry.controller.signal.throwIfAborted()
      await preflight?.(entry.record,entry.controller.signal)
      entry.controller.signal.throwIfAborted()
      await this.withOwner(owner,async() => {
        entry.controller.signal.throwIfAborted()
        try { await store.save(entry.record) }
        catch { throw new RunPersistenceError('initial',entry.record) }
        entry.saved = true
        entry.claimed = false
      })
      entry.controller.signal.throwIfAborted()
      await onInitial?.(entry.record)
      entry.controller.signal.throwIfAborted()
      const launched = launch(entry.record,entry.controller.signal)
      entry.launched = true
      entry.result = this.collect(entry,launched)
      entry.result.catch(() => {})
      // The adapter calls publish only beside its successful tool return, after formatting it.
      return entry
    } catch (error) {
      try { await this.fail(entry,error) }
      finally { entry.release() }
      if (!(error instanceof RunPersistenceError) && entry.record.state === 'stopped') throw Error(entry.record.error)
      throw error
    }
  }
}
