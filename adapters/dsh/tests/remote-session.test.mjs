import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Context } from '@deepseek-ai/cordis'
import SessionStore from '@deepseek-ai/dsh-session'
import { openRemoteSession } from '../src/remote-session.js'

test('remote transcript is a real parent-linked session with persisted input and result', async () => {
 const ctx = new Context()
 try {
  await ctx.plugin(SessionStore)
  const session = ctx.sessions.create('parent', {meta:{cwd:process.cwd()}})
  const transcript = openRemoteSession(ctx,{session},'smart-home-agent',[{type:'text',text:'query temperature'}],'remote')
  const child = ctx.sessions.get(transcript.id)
  assert.equal(child.header.parentSession,session.id)
  assert.equal(child.header.origin,'subagent')
  transcript.close({output:[{type:'text',text:'23°C'}],stopReason:'completed'})
  assert.equal(child.eventAt(0).type,'session/title')
  const events = Array.from({length:child.seq},(_,i)=>child.eventAt(i))
  assert.equal(events.filter(e=>e.type==='user/message').length,1)
  assert.equal(events.find(e=>e.type==='assistant/message').data.message.content[0].text,'23°C')
  assert.equal(events.at(-1).type,'turn/end')
  assert.equal(session.eventAt(session.seq-1).data.childId,transcript.id)
 } finally {await ctx.fiber.dispose()}
})
