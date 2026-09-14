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
  transcript.progress({stage:'request',method:'message/send',origin:'https://secret.invalid'})
  transcript.progress({stage:'task',taskId:'task-1',state:'working'})
  transcript.progress({stage:'task',taskId:'task-1',state:'working'})
  transcript.progress({stage:'remote_status',text:'[模拟工具调用] demo.get_device_status'})
  for (let i=0;i<3;i++) {
    transcript.progress({stage:'request',method:'tasks/get'})
    transcript.progress({stage:'response',method:'tasks/get',elapsedMs:1})
  }
  transcript.close({output:[{type:'text',text:'23°C'}],stopReason:'completed'})
  assert.equal(child.events[0].type,'session/title')
  const events = child.events
  assert.equal(events.filter(e=>e.type==='user/message' && e.data.source.kind==='user').length,1)
  assert.equal(events.find(e=>e.type==='user/message').data.source.kind,'user')
  const messages = events.filter(e=>e.type==='user/message' && e.data.source.kind==='plugin').map(e=>e.data.content[0].text)
  assert.ok(messages.includes('23°C'))
  assert.ok(messages.some(text=>text.includes('[模拟工具调用]')))
  assert.equal(messages.filter(text=>text.includes('A2A 请求：tasks/get')).length,1)
  assert.ok(messages.some(text=>text.includes('共 3 次任务查询')))
  assert.equal(messages.filter(text=>text.includes('状态：working')).length,1)
  assert.ok(messages.some(text=>text.includes('message/send')))
  assert.ok(!JSON.stringify(events).includes('secret.invalid'))
  assert.equal(events.at(-1).type,'turn/end')
  assert.equal(child.header.parentSession, session.id)
  assert.ok(!session.events.some(event=>event.type==='subagent/catalog'))
 } finally {await ctx.fiber.dispose()}
})
