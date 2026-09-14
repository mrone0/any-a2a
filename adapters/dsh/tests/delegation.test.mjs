import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Context } from '@deepseek-ai/cordis'
import SessionStore from '@deepseek-ai/dsh-session'
import { apply } from '../src/delegation-tool.js'
test('omitted flag starts native background job; false waits', async t => {
  const host = new Context(); t.after(() => host.fiber.dispose()); await host.plugin(SessionStore)
  let tool, job
  let disposed = 0
  const ctx = {
    sessions:host.sessions,
    effect: fn => fn(), tools:{register:t=>{tool=t;return ()=>{}}},
    jobs:{start:spec=>{job=spec;return 'job-1'}},
    subagents:{getProvider:()=>({}),start:async()=>({result:Promise.resolve({output:[],stopReason:'completed'}),dispose:async()=>{disposed++}})},
  }
  apply(ctx,{provider:'remote',toolName:'specialist'})
  const exec={agent:{id:'parent',session:host.sessions.create('parent',{meta:{cwd:process.cwd()}})},signal:new AbortController().signal}
  const args={description:'read',prompt:'query'}
  const started = await tool.execute(args,exec)
  assert.equal(started.kind,'background'); assert.equal(started.jobId,'job-1')
  assert.equal(host.sessions.get(started.sessionId).header.parentSession,'parent')
  assert.equal(job.kind,'subagent');assert.equal(job.owner,exec.agent)
  const running=job.run();await running.done;assert.equal(disposed,1)
  assert.equal((await tool.execute({...args,run_in_background:false},exec)).kind,'foreground')
  assert.equal(disposed,2)
})
