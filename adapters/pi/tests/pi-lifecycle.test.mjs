import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createServer } from 'node:http'
import { randomUUID } from 'node:crypto'
import { RunStore } from '../runs.mjs'
import { assertFixtureChild, removeFixtureDirectory } from './temp-dir.mjs'

const root=process.env.PI_TEST_ROOT
const executable=process.env.ANY_A2A_TEST_BINARY || fileURLToPath(new URL('../../../target/debug/any-a2a'+(process.platform==='win32'?'.exe':''),import.meta.url))

async function fixture(run) {
  const dir=await mkdtemp(join(tmpdir(),'pi-host-lifecycle-')),agentDir=join(dir,'agent')
  await mkdir(agentDir)
  const keys=['ANY_A2A_EXECUTABLE','ANY_A2A_DATA_DIR','PI_CODING_AGENT_DIR']
  const previous=Object.fromEntries(keys.map(key=>[key,process.env[key]]))
  process.env.ANY_A2A_EXECUTABLE=executable;process.env.ANY_A2A_DATA_DIR=dir;process.env.PI_CODING_AGENT_DIR=agentDir
  const hosts=[],requests=[],notices=[]
  const server=createServer(async(req,res)=>{
    assert.equal(req.headers.authorization,'Bearer lifecycle-fixture-only')
    let body='';for await(const chunk of req)body+=chunk
    const rpc=JSON.parse(body);requests.push(rpc)
    const task=rpc.method==='message/send'?rpc.params.message.parts[0].text:rpc.params.id
    const result=task.startsWith('hold')
      ? {kind:'task',id:task,status:{state:'working',message:{kind:'message',parts:[{kind:'text',text:'fixture waiting'}]}}}
      : {kind:'message',parts:[{kind:'text',text:'PI-LIFECYCLE-OK'}]}
    res.setHeader('content-type','application/json');res.end(JSON.stringify({jsonrpc:'2.0',id:rpc.id,result}))
  })
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve))
  try {
    const pkg=JSON.parse(await readFile(join(root,'package.json'),'utf8'))
    assert.equal(pkg.version,'0.85.1','native lifecycle acceptance is pinned to the inspected Pi API')
    const {loadExtensions}=await import(pathToFileURL(join(root,'dist/core/extensions/loader.js')).href)
    const {ExtensionRunner,SessionManager}=await import(pathToFileURL(join(root,'dist/index.js')).href)
    const card={name:'lifecycle-fixture',description:'fixture',version:'1',protocolVersion:'0.3.0',url:`http://127.0.0.1:${server.address().port}/rpc`,capabilities:{},defaultInputModes:['text'],defaultOutputModes:['text'],skills:[]}
    await writeFile(join(dir,'agent-cards.jsonl'),JSON.stringify({id:'fixture',source:'manual',agentCard:card,auth:{bearerToken:'lifecycle-fixture-only'}})+'\n')
    const session=SessionManager.create(dir,join(dir,'sessions'))
    const load=async(sessionManager=session)=>{
      const loaded=await loadExtensions([fileURLToPath(new URL('../index.ts',import.meta.url))],dir)
      assert.deepEqual(loaded.errors,[])
      const runner=new ExtensionRunner(loaded.extensions,loaded.runtime,dir,sessionManager,{})
      // Public host bindings capture notices without running a model or fabricating messages.
      runner.bindCore({sendMessage:(...args)=>notices.push(args),sendUserMessage(){},appendEntry(){},setSessionName(){},getSessionName(){},setLabel(){},getActiveTools:()=>[],getAllTools:()=>[],setActiveTools(){},refreshTools(){},getCommands:()=>[],setModel:async()=>false,getThinkingLevel:()=> 'off',setThinkingLevel(){}},
        {getModel:()=>undefined,getScopedModels:()=>[],isIdle:()=>true,isProjectTrusted:()=>true,getSignal:()=>undefined,abort(){},hasPendingMessages:()=>false,shutdown(){},getContextUsage:()=>undefined,compact(){},getSystemPrompt:()=>''})
      const errors=[];runner.onError(error=>errors.push(error))
      const host={runner,errors,session:sessionManager,tools:loaded.extensions[0].tools,ctx:runner.createContext()}
      hosts.push(host)
      await runner.emit({type:'session_start'})
      return host
    }
    const host=await load(),store=new RunStore(join(agentDir,'a2a-runs'),session.getSessionId())
    await run({dir,host,store,requests,notices,load,SessionManager})
  }finally{
    try {
      for(const host of hosts) {
        await host.runner.emit({type:'session_shutdown',reason:'quit'})
      }
      server.closeAllConnections();await new Promise(resolve=>server.close(resolve))
      await assertFixtureChild(dir,agentDir)
      await removeFixtureDirectory(dir,'pi-host-lifecycle-')
      assert.deepEqual(hosts.flatMap(host=>host.errors),[],'Pi runner swallows handler errors; inspect its error channel')
    }finally{for(const key of keys)if(previous[key]===undefined)delete process.env[key];else process.env[key]=previous[key]}
  }
}

const tool=(host,name)=>host.tools.get(name).definition
const delegate=(host,id,task,signal=new AbortController().signal,update,background=true)=>tool(host,'a2a_delegate').execute(id,{agentId:'fixture',task,background},signal,update,host.ctx)

test('Pi 0.85.1 native loader: atomic parallel admission, background handoff and local stop',{skip:!root},async()=>{
  await fixture(async({host,store,requests})=>{
    const controllers=Array.from({length:6},()=>new AbortController())
    const results=await Promise.allSettled(controllers.map((controller,i)=>delegate(host,'parallel-'+i,'hold-'+i,controller.signal)))
    const accepted=results.filter(result=>result.status==='fulfilled').map(result=>result.value)
    assert.equal(accepted.length,4)
    assert.equal(results.filter(result=>result.status==='rejected' && /At most 4/.test(result.reason.message)).length,2)
    controllers.forEach(controller=>controller.abort())
    for(const result of accepted) {
      const id=result.details.runId
      assert.equal((await tool(host,'a2a_runs').execute('inspect',{runId:id},undefined,undefined,host.ctx)).details.record.state,'running')
      await tool(host,'a2a_stop').execute('stop',{runId:id},undefined,undefined,host.ctx)
      const record=await store.get(id)
      assert.equal(record.state,'stopped');assert.ok(record.endedAt)
      assert.notEqual(record.remoteState,'canceled')
    }
    assert.ok(!requests.some(request=>request.method==='tasks/cancel'))
  })
})

test('Pi 0.85.1 native loader: initial abort, update exception and synchronous launch failure persist terminal records',{skip:!root},async()=>{
  await fixture(async({dir,host,store,requests})=>{
    const abort=new AbortController()
    await assert.rejects(delegate(host,'abort','not-submitted',abort.signal,()=>abort.abort(),false),/Local delegation stopped/)
    await assert.rejects(delegate(host,'update-throw','not-submitted',undefined,()=>{throw Error('fixture update failed')},false),/fixture update failed/)
    try {
      await assert.rejects(delegate(host,'sync-launch','not-submitted',undefined,()=>{process.env.ANY_A2A_DATA_DIR='relative-fixture-data'},false),/Invalid any-a2a deployment paths/)
    }finally{process.env.ANY_A2A_DATA_DIR=dir}
    const records=await store.list()
    assert.deepEqual(records.map(record=>record.state).sort(),['failed','failed','stopped'])
    assert.ok(records.every(record=>record.endedAt && record.updatedAt===record.endedAt))
    assert.equal(requests.length,0,'no A2A submission after startup failure')
    const next=await delegate(host,'after-failures','success',undefined,undefined,false)
    assert.match(next.content[0].text,/PI-LIFECYCLE-OK/)
  })
})

test('Pi 0.85.1 public runner: shutdown waits pending starts and session_start restores admission',{skip:!root},async()=>{
  await fixture(async({host,requests})=>{
    const pending=delegate(host,'pending','hold-pending',undefined,undefined,false).catch(error=>error)
    await host.runner.emit({type:'session_shutdown',reason:'reload'})
    assert.match((await pending).message,/Local delegation stopped/)
    assert.equal(requests.length,0)
    await assert.rejects(delegate(host,'closed','not-submitted'),/shutting down/)
    await host.runner.emit({type:'session_start'})
    const result=await delegate(host,'resumed','success',undefined,undefined,false)
    assert.match(result.content[0].text,/PI-LIFECYCLE-OK/)
    assert.deepEqual(host.errors,[])
  })
})

test('Pi 0.85.1 public runner: owned persisted reload interrupts orphans without resubmitting',{skip:!root},async()=>{
  await fixture(async({dir,host,store,requests,load,SessionManager})=>{
    const result=await delegate(host,'complete','success',undefined,undefined,false)
    const orphan={version:1,id:randomUUID(),owner:store.owner,agentId:'fixture',task:'orphan',background:false,state:'running',startedAt:Date.now(),updatedAt:Date.now(),progress:[]}
    await store.save(orphan)
    host.session.appendMessage({role:'user',content:'fixture input',timestamp:Date.now()})
    host.session.appendMessage({role:'toolResult',toolCallId:'complete',toolName:'a2a_delegate',...result,isError:false,timestamp:Date.now()})
    const sessionFile=join(dir,'reload.jsonl')
    await writeFile(sessionFile,[host.session.getHeader(),...host.session.getEntries()].map(row=>JSON.stringify(row)).join('\n')+'\n')
    await host.runner.emit({type:'session_shutdown',reason:'reload'})
    const reloaded=await load(SessionManager.open(sessionFile))
    const completed=await tool(reloaded,'a2a_runs').execute('completed',{runId:result.details.runId},undefined,undefined,reloaded.ctx)
    assert.equal(completed.details.record.state,'completed')
    const interrupted=await tool(reloaded,'a2a_runs').execute('orphan',{runId:orphan.id},undefined,undefined,reloaded.ctx)
    assert.equal(interrupted.details.record.state,'interrupted');assert.ok(interrupted.details.record.endedAt)
    assert.equal(requests.filter(request=>request.method==='message/send').length,1)
    const other=await load(SessionManager.create(dir,join(dir,'other-sessions')))
    await assert.rejects(tool(other,'a2a_runs').execute('wrong',{runId:result.details.runId},undefined,undefined,other.ctx))
    await assert.rejects(tool(other,'a2a_stop').execute('wrong-stop',{runId:result.details.runId},undefined,undefined,other.ctx))
  })
})
