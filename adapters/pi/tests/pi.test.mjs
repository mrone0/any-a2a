import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { resolve, join } from 'node:path'
import { pathToFileURL, fileURLToPath } from 'node:url'
import { createServer } from 'node:http'
import { deployment, executeCli, progressText } from '../transport.mjs'

test('deployment strips ambient credentials; progress hides origins', () => {
  const config=deployment({PATH:'x',MODEL_API_KEY:'secret',ANY_A2A_TOKEN:'secret',ANY_A2A_EXECUTABLE:'native'})
  assert.deepEqual(config.env,{PATH:'x',ANY_A2A_EXECUTABLE:'native'})
  assert.equal(progressText({stage:'response',origin:'secret'}),null)
  assert.match(progressText({stage:'remote_status',text:'[模拟进度]'}),/模拟/)
})

test('spawn errors and pre-abort are failures, never completion',async()=>{
  await assert.rejects(executeCli(['catalog'],{config:{executable:'no-such-any-a2a-executable',env:{}}}),/Cannot start/)
  const controller=new AbortController();controller.abort()
  assert.throws(()=>executeCli(['catalog'],{signal:controller.signal}))
})

const root=process.env.PI_TEST_ROOT
// Explicit installed Pi build, not a fake SDK; no model API key needed.
test('actual Pi loader, native CLI, live progress, and persisted tool details round trip',{skip:!root},async()=>{
  const {loadExtensions}=await import(pathToFileURL(join(root,'dist/core/extensions/loader.js')).href)
  const {SessionManager}=await import(pathToFileURL(join(root,'dist/core/session-manager.js')).href)
  const dir=await mkdtemp(join(tmpdir(),'pi-a2a-'))
  const previous={exe:process.env.ANY_A2A_EXECUTABLE,data:process.env.ANY_A2A_DATA_DIR}
  process.env.ANY_A2A_EXECUTABLE=process.env.ANY_A2A_TEST_BINARY||fileURLToPath(new URL('../../../target/demo-build/debug/any-a2a'+(process.platform==='win32'?'.exe':''),import.meta.url))
  process.env.ANY_A2A_DATA_DIR=dir
  let runDirectory
  let polls=0
  const server=createServer(async(req,res)=>{
    assert.equal(req.headers.authorization,'Bearer fixture-secret')
    let body='';for await(const chunk of req)body+=chunk
    const q=JSON.parse(body)
    const done=q.method==='tasks/get'&&++polls>=2
    const result={kind:'task',id:'task-1',status:{state:done?'completed':'working',message:{kind:'message',parts:[{kind:'text',text:'[模拟进度] fixture only'}]}},artifacts:done?[{parts:[{kind:'text',text:'PI-A2A-OK'}]}]:[]}
    res.setHeader('content-type','application/json');res.end(JSON.stringify({jsonrpc:'2.0',id:q.id,result}))
  })
  await new Promise(r=>server.listen(0,'127.0.0.1',r))
  try {
    const card={name:'fixture',description:'test',version:'1',protocolVersion:'0.3.0',url:`http://127.0.0.1:${server.address().port}/rpc`,capabilities:{},defaultInputModes:['text'],defaultOutputModes:['text'],skills:[]}
    await writeFile(join(dir,'agent-cards.jsonl'),JSON.stringify({id:'fixture',source:'manual',agentCard:card,auth:{bearerToken:'fixture-secret'}})+'\n')
    const loaded=await loadExtensions([fileURLToPath(new URL('../index.ts',import.meta.url))],process.cwd())
    assert.deepEqual(loaded.errors,[])
    const extension=loaded.extensions[0]
    const tool=extension.tools.get('a2a_delegate').definition
    const updates=[]
    const ctx={sessionManager:{getSessionId:()=> 'pi-a2a-test-'+dir.split(/[\\/]/).at(-1)}}
    const {getAgentDir}=await import(pathToFileURL(join(root,'dist/config.js')).href)
    const {RunStore}=await import('../runs.mjs')
    runDirectory=new RunStore(join(getAgentDir(),'a2a-runs'),ctx.sessionManager.getSessionId()).dir
    const result=await tool.execute('call-1',{agentId:'fixture',task:'test 中文'},new AbortController().signal,u=>updates.push(u),ctx)
    assert.match(result.content[0].text,/PI-A2A-OK/)
    assert.match(result.content[0].text,/模拟进度/)
    assert.match(result.content[0].text,/不代表已验证客户端实时渲染/)
    assert.equal(result.details.state,'completed')
    assert.ok(updates.some(u=>JSON.stringify(u).includes('模拟进度')))
    assert.ok(!JSON.stringify(result).includes('fixture-secret'))
    const sm=SessionManager.create(dir,join(dir,'sessions'))
    sm.appendMessage({role:'user',content:'test',timestamp:Date.now()})
    sm.appendMessage({role:'toolResult',toolCallId:'call-1',toolName:'a2a_delegate',...result,isError:false,timestamp:Date.now()})
    const entries=sm.getEntries()
    // Save SDK-generated records to a fixture: real Pi persists these during its agent loop.
    const sessionFile=join(dir,'roundtrip.jsonl')
    await writeFile(sessionFile,[sm.getHeader(),...entries].map(row=>JSON.stringify(row)).join('\n')+'\n')
    const restored=SessionManager.open(sessionFile).getEntries().at(-1).message
    assert.equal(restored.details.state,'completed')
    assert.match(restored.content[0].text,/模拟进度/)
    assert.deepEqual(restored.details.progress,result.details.progress)
    const abort=new AbortController()
    const pending=tool.execute('call-2',{agentId:'fixture',task:'abort'},abort.signal,()=>abort.abort(),ctx)
    await assert.rejects(pending,/Local delegation stopped/)
    const background=await tool.execute('call-3',{agentId:'fixture',task:'background',background:true},new AbortController().signal,undefined,ctx)
    assert.equal(background.details.state,'running')
    const inspect=extension.tools.get('a2a_runs').definition
    let snapshot
    for(let i=0;i<30;i++) {
      snapshot=await inspect.execute('inspect',{runId:background.details.runId},undefined,undefined,ctx)
      if(snapshot.details.record.state!=='running')break
      await new Promise(r=>setTimeout(r,100))
    }
    assert.equal(snapshot.details.record.state,'completed')
    await assert.rejects(inspect.execute('wrong',{runId:background.details.runId},undefined,undefined,{sessionManager:{getSessionId:()=> 'other-parent'}}))
  } finally {
    await new Promise(r=>server.close(r));await rm(dir,{recursive:true,force:true})
    if(runDirectory)await rm(runDirectory,{recursive:true,force:true})
    for(const [key,value]of [['ANY_A2A_EXECUTABLE',previous.exe],['ANY_A2A_DATA_DIR',previous.data]])if(value===undefined)delete process.env[key];else process.env[key]=value
  }
})
