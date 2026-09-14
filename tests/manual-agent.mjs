// Temporary loopback-only A2A fixture. Not an LLM; no external side effects.
import { createServer } from 'node:http'
import { writeFileSync } from 'node:fs'
import { randomUUID } from 'node:crypto'
const tasks = new Map()
let base
const stages = [
  '[模拟进度 1/4] 已收到任务，正在解析请求。这是预设演示，不是真实模型思考。',
  '[模拟进度 2/4] 准备查询测试设备；没有访问真实设备。',
  '[模拟工具调用] demo.get_device_status\n\n输入：\n```json\n{"deviceId":"demo-01"}\n```\n\n输出（固定测试数据）：\n```json\n{"online":true,"temperature":23,"simulated":true}\n```',
  '[模拟进度 4/4] 已取得固定测试数据，正在整理最终输出。',
]
function publicTask(task) {
  const {text,started,...wire}=task
  return wire
}
const server = createServer(async (req, res) => {
  res.setHeader('Content-Type', 'application/json')
  const send = value => res.end(JSON.stringify(value))
  if (req.method === 'GET' && req.url === '/.well-known/agent-card.json') return send({
    name:'临时 A2A 验证 Agent',description:'本地测试专员：返回 A2A-DEMO-OK 标记及请求回显，验证子 Agent 委派链路。无真实设备操作。',
    version:'1.0',protocolVersion:'0.3.0',url:`${base}/rpc`,preferredTransport:'JSONRPC',capabilities:{},
    defaultInputModes:['text/plain'],defaultOutputModes:['text/plain'],
    skills:[{id:'verify',name:'验证 A2A 调用',description:'运行本地链路测试并返回 A2A-DEMO-OK；不是实时外部数据。',tags:['test','verification']}],
  })
  if (req.method !== 'POST' || req.url !== '/rpc') {res.statusCode=404;return send({error:'not found'})}
  let bytes=''
  for await (const chunk of req) {bytes+=chunk;if(bytes.length>65536){res.statusCode=413;return send({error:'too large'})}}
  let q
  try {q=JSON.parse(bytes)} catch {res.statusCode=400;return send({error:'invalid JSON'})}
  const reply = result => send({jsonrpc:'2.0',id:q.id,result})
  console.log(JSON.stringify({method:q.method,at:new Date().toISOString()}))
  if(q.method==='message/send') {
    const id=randomUUID(), text=(q.params?.message?.parts||[]).filter(p=>p.kind==='text').map(p=>p.text).join('')
    tasks.set(id,{kind:'task',id,contextId:randomUUID(),status:{state:'working',message:{kind:'message',role:'agent',messageId:randomUUID(),parts:[{kind:'text',text:stages[0]}]}},artifacts:[],text,started:Date.now()})
    return reply(publicTask(tasks.get(id)))
  }
  const task=tasks.get(q.params?.id)
  if(!task)return send({jsonrpc:'2.0',id:q.id,error:{code:-32001,message:'Task not found'}})
  if(q.method==='tasks/cancel') {
    if(task.status.state==='working')task.status={state:'canceled',message:{kind:'message',role:'agent',messageId:randomUUID(),parts:[{kind:'text',text:'[模拟任务] 已取消，不再执行后续模拟步骤。'}]}}
  }
  else if(q.method==='tasks/get') {
    if(task.status.state==='working') {
      const stage=Math.floor((Date.now()-task.started)/1800)
      if(stage>=stages.length) {
        task.status={state:'completed'}
        task.artifacts=[{artifactId:'verification',parts:[{kind:'text',text:`A2A-DEMO-OK\n真实 A2A HTTP 链路验证成功。\n模拟结果：demo-01 在线，温度 23°C（固定测试数据，未查询真实设备）。\n收到任务：${task.text}` }]}]
      } else {
        task.status.message={kind:'message',role:'agent',messageId:`${task.id}-stage-${stage}`,parts:[{kind:'text',text:stages[stage]}]}
      }
    }
  }else return send({jsonrpc:'2.0',id:q.id,error:{code:-32601,message:'Method not found'}})
  reply(publicTask(task))
})
server.listen(Number(process.argv[3] || 0),'127.0.0.1',()=>{base=`http://127.0.0.1:${server.address().port}`;writeFileSync(process.argv[2],JSON.stringify({pid:process.pid,cardUrl:`${base}/.well-known/agent-card.json`}));console.log(base)})
