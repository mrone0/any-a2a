/** Durable transcript for a remote one-shot delegation; not a local LLM Agent. */
import { randomUUID } from 'node:crypto'
import { createMessage, createUserMessage } from '@deepseek-ai/dsh-llm'

export function openRemoteSession(ctx, parent, label, prompt, provider) {
  if (!ctx.sessions || !parent.session) throw Error('Remote child transcript requires sessions and a parent session')
  const id = randomUUID()
  const child = ctx.sessions.create(id, {meta:{
    origin:'subagent', parentSession:parent.session.id,
    cwd:parent.session.header.cwd,
    delegationDepth:(parent.session.header.delegationDepth ?? 0)+1,
  }})
  child.append('session/title',{title:label,messageSeqs:[],source:{kind:'user'}})
  child.append('turn/start',{turn:1})
  child.append('subagent/descriptor',{version:3,mode:'one-shot',provider,label})
  child.append('user/message',createUserMessage({content:prompt,source:{kind:'plugin',plugin:'any-a2a'}}), {surfaceOp:'append'})
  parent.session.append('subagent/catalog',{version:0,childId:id,childCreatedAt:child.header.createdAt,mode:'one-shot',label})
  let closed = false
  return {
    id,
    close(result) {
      if (closed) return
      closed = true
      const output = result.output?.length ? result.output : [{type:'text',text:`远端委派结束：${result.stopReason}。未提供更多执行详情。`}]
      child.append('step/start',{turn:1,step:1})
      child.append('assistant/message',{turn:1,step:1,stream:[],message:createMessage({role:'assistant',content:output,source:{kind:'plugin',plugin:'any-a2a'}})}, {surfaceOp:'append'})
      child.append('step/end',{turn:1,step:1})
      child.append('turn/end',{turn:1,reason:result.stopReason === 'completed' ? {kind:'completed'} : {kind:'error',error:{code:'REMOTE_DELEGATION',message:`Remote delegation: ${result.stopReason}`}}})
    },
  }
}
