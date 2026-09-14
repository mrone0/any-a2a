/** Durable transcript for a remote one-shot delegation; not a local LLM Agent. */
import { SUBAGENT_DESCRIPTOR_VERSION } from '@deepseek-ai/dsh-subagent'
import { randomUUID } from 'node:crypto'
import { createUserMessage } from '@deepseek-ai/dsh-llm'

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
  child.append('subagent/descriptor',{version:SUBAGENT_DESCRIPTOR_VERSION,mode:'one-shot',provider,label})
  child.append('user/message',createUserMessage({content:prompt,source:{kind:'user'}}), {surfaceOp:'append'})
  let closed = false
  let step = 0
  let lastTask = ''
  let polls = 0
  const started = Date.now()
  const append = text => {
    step++
    child.append('step/start',{turn:1,step})
    child.append('user/message',createUserMessage({content:[{type:'text',text}],source:{kind:'plugin',plugin:'any-a2a'}}), {surfaceOp:'append'})
    child.append('step/end',{turn:1,step})
  }
  return {
    id,
    progress(event) {
      if (closed) return
      const method = ['message/send','tasks/get','tasks/cancel','SendMessage','GetTask','CancelTask'].includes(event.method) ? event.method : 'A2A'
      if (method === 'tasks/get' || method === 'GetTask') {
        if (event.stage === 'request') { polls++; if (polls > 1) return }
        if (event.stage === 'response' && polls > 1 && event.rpcErrorCode == null) return
      }
      if (event.stage === 'request') append(`A2A 请求：${method}（协议通信，不是远端内部工具调用）`)
      else if (event.stage === 'response') append(`A2A 响应：${method} · ${Number(event.elapsedMs) || 0} ms${event.rpcErrorCode != null ? ' · 远端协议错误' : ''}`)
      else if (event.stage === 'transport_error') append(`A2A 传输失败：${method}；未自动重试，远端状态未知。`)
      else if (event.stage === 'remote_status' && typeof event.text === 'string') {
        append(`远端进度消息（远端提供的内容，非本机执行记录）：\n\n${event.text}`)
      }
      else if (event.stage === 'task') {
        const key = `${event.taskId}:${event.state}`
        if (key !== lastTask) { lastTask = key; append(`远端任务：${event.taskId}\n状态：${event.state}`) }
      }
    },
    close(result) {
      if (closed) return
      closed = true
      const output = result.output?.length ? result.output : [{type:'text',text:`远端委派结束：${result.stopReason}。未提供更多执行详情。`}]
      const rendered = output.map(block => {
        if (block.type !== 'text') return block
        try {
          const raw = JSON.parse(block.text)
          const parts = [...(raw.parts || []), ...(raw.artifacts || []).flatMap(a => a.parts || [])]
          const text = parts.filter(p => typeof p.text === 'string').map(p => p.text).join('')
          return {type:'text',text: text ? `${text}\n\n原始 A2A 结果：\n\n\`\`\`json\n${JSON.stringify(raw,null,2)}\n\`\`\`` : block.text}
        } catch { return block }
      })
      step++
      child.append('step/start',{turn:1,step})
      child.append('user/message',createUserMessage({content:rendered,source:{kind:'plugin',plugin:'any-a2a'}}), {surfaceOp:'append'})
      child.append('step/end',{turn:1,step})
      if (polls > 1) append(`轮询汇总：共 ${polls} 次任务查询；重复的成功查询已合并，状态变化与远端进度单独展示。`)
      append(`委派结束：${result.stopReason} · ${((Date.now()-started)/1000).toFixed(1)} 秒。远端未提供内部思考或工具执行轨迹。`)
      child.append('turn/end',{turn:1,reason:result.stopReason === 'completed' ? {kind:'completed'} : {kind:'error',error:{code:'REMOTE_DELEGATION',message:`Remote delegation: ${result.stopReason}`}}})
    },
  }
}
