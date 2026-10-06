import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createServer } from 'node:http'
import { executeCli as piCli, deployment } from '../../pi/transport.mjs'
import { executeCli as opencodeCli } from '../../opencode/transport.mjs'
import { createProvider } from '../../dsh/src/index.js'
import { observeProgress } from '../../dsh/src/progress.js'

// Deliberately real native Rust CLI. No desktop service, remote credentials or host SDK.
const executable=process.env.ANY_A2A_TEST_BINARY || fileURLToPath(new URL('../../../target/debug/any-a2a'+(process.platform==='win32'?'.exe':''),import.meta.url))
const maxResponse=4*1024*1024
const nearUnit='"\\\n中🙂'
const encodedUnitBytes=Buffer.byteLength(JSON.stringify(nearUnit))-2
const nearText=nearUnit.repeat(Math.floor((maxResponse-2048)/encodedUnitBytes))
const texts={medium:'x'.repeat(600*1024),near:nearText,progress:'完整最终输出：中文🙂'}
const message=text=>({kind:'message',parts:[{kind:'text',text},{kind:'data',data:{fixture:true,unchanged:[1,'two',null]}}]})

for(const scenario of ['medium','near','progress']) {
  test(`real CLI budgets preserve ${scenario} output across Pi, OpenCode and DSH`,{timeout:30000},async()=>{
    const directory=await mkdtemp(join(tmpdir(),'a2a-budget-'))
    const sends=new Map(),polls=new Map(),expected=new Map()
    const server=createServer(async(req,res)=>{
      let body='';for await(const chunk of req)body+=chunk
      const rpc=JSON.parse(body)
      const sending=rpc.method==='message/send'
      const route=sending?rpc.params.message.parts[0].text:rpc.params.id
      let result
      if(sending){sends.set(route,(sends.get(route)||0)+1);polls.set(route,0)}
      if(scenario==='progress') {
        const count=sending?0:(polls.get(route)||0)+1
        polls.set(route,count)
        const completed=count>=40
        result={kind:'task',id:route,status:{state:completed?'completed':'working',message:{kind:'message',parts:[{kind:'text',text:completed?'done':count+' '+'p'.repeat(256*1024)}]}},artifacts:completed?[{parts:message(texts.progress).parts}]:[]}
        if(completed)expected.set(route,result)
      }else{result=message(texts[scenario]);expected.set(route,result)}
      const response=JSON.stringify({jsonrpc:'2.0',id:rpc.id,result})
      assert.ok(Buffer.byteLength(response)<maxResponse,'fixture is a legal bounded HTTP response')
      if(scenario==='near')assert.ok(Buffer.byteLength(response)>maxResponse-8192,'exercise nearly 4 MiB, including JSON escaping')
      res.setHeader('content-type','application/json');res.end(response)
    })
    await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve))
    try {
      const card={name:'budget-fixture',description:'fixture',version:'1',protocolVersion:'0.3.0',url:`http://127.0.0.1:${server.address().port}/rpc`,capabilities:{},defaultInputModes:['text'],defaultOutputModes:['text'],skills:[]}
      await writeFile(join(directory,'agent-cards.jsonl'),JSON.stringify({id:'saved',source:'manual',agentCard:card})+'\n')
      const config=deployment({ANY_A2A_EXECUTABLE:executable,ANY_A2A_DATA_DIR:directory})
      const results=await Promise.all(['pi','opencode','dsh'].map(async client=>{
        const route=`${scenario}-${client}`,updates=[]
        const onProgress=event=>updates.push(event)
        let outcome
        if(client==='dsh'){
          const controller=new AbortController(),unobserve=observeProgress(controller.signal,onProgress)
          let run
          try {
            run=await createProvider({agentId:'saved',dataDir:directory,executable}).start({prompt:[{type:'text',text:route}],signal:controller.signal})
            const result=await run.result
            assert.equal(result.stopReason,'completed')
            outcome={raw:JSON.parse(result.output[0].text),text:texts[scenario],state:'completed'}
          }finally{await run?.dispose();unobserve()}
        }else{
          outcome=await (client==='pi'?piCli:opencodeCli)(['run','--agent-id','saved','--events','--message',route],{config,onProgress})
          assert.equal(outcome.state,'completed')
          assert.equal(outcome.text,texts[scenario])
        }
        assert.deepEqual(outcome.raw,expected.get(route),'full structured raw result is retained')
        assert.equal(sends.get(route),1,'no automatic task resubmission')
        if(scenario==='progress'){
          assert.equal(polls.get(route),40,'changing status does not abort collection early')
          const progress=updates.filter(event=>event.stage==='remote_status')
          assert.ok(progress.length>=40)
          assert.ok(progress.every(event=>Buffer.byteLength(event.text)<=8192),'producer bounds each status before adapters receive it')
          if(client!=='dsh')assert.equal(outcome.progress_truncated,true)
        }
        return client
      }))
      assert.deepEqual(results,['pi','opencode','dsh'])
    }finally{
      server.closeAllConnections();await new Promise(resolve=>server.close(resolve))
      await rm(directory,{recursive:true,force:true})
    }
  })
}
