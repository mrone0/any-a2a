import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { setTimeout as delay } from 'node:timers/promises'
import { launchCli, executeCli, CliTransportError } from '../src/cli-transport.js'

const fixture=fileURLToPath(new URL('./fixture-cli.mjs',import.meta.url))
const options=(mode,pidFile)=>({config:{executable:process.execPath,args:[fixture,mode,pidFile || '-'],env:{}}})
const args=['run','--events']
const safeError=kind=>error=>error instanceof CliTransportError && error.kind===kind && !/secret-token|private-prompt/.test(error.message)

test('native prefix invocation, UTF-8 framing and complete raw results',async()=>{
  const updates=[]
  const result=await executeCli(args,{...options('split-unicode'),onProgress:event=>updates.push(event)})
  assert.equal(result.text,'中文🙂')
  assert.equal(result.raw.parts[0].text,result.text)
  assert.equal(updates[0].stage,'request')
  const plain=await executeCli(['run'],options('plain'))
  assert.deepEqual(plain,result)
  const truncated=[]
  await executeCli(args,{...options('valid'),onProgress:event=>truncated.push(event)})
  assert.equal(truncated[0].stage,'progress_truncated')
})

test('publication precedes completion, and final result waits for process close',async()=>{
  let observed
  const progress=new Promise(resolve=>observed=resolve)
  const handle=launchCli(args,{...options('delay-close'),onProgress:()=>observed()})
  const result=handle.result
  let settled=false
  result.finally(()=>settled=true)
  await handle.spawned
  await progress
  await delay(25)
  assert.equal(settled,false)
  assert.equal((await result).state,'completed')
  handle.stop()
  assert.equal((await result).state,'completed')
})

test('malformed streams, invalid UTF-8 and nonzero exits stay sanitized',async()=>{
  for(const mode of ['invalid','utf8','duplicate','after-result','incomplete','fields','no-result']) {
    await assert.rejects(executeCli(args,options(mode)),safeError('protocol'),mode)
  }
  await assert.rejects(executeCli(args,options('nonzero')),safeError('exit'))
  await assert.rejects(executeCli(args,{...options('valid'),onProgress(){throw Error('secret-token private-prompt')}}),safeError('protocol'))
})

test('progress, frame and total output limits are checked independently',async()=>{
  await assert.rejects(executeCli(args,options('large-progress')),safeError('output_limit'))
  await assert.rejects(executeCli(args,{...options('valid'),maxFrameBytes:100}),safeError('output_limit'))
  await assert.rejects(executeCli(args,{...options('overflow'),maxOutputBytes:2000}),safeError('output_limit'))
})

test('trusted prefixes are bounded and oversized safe-integer deadlines do not overflow timers',async()=>{
  assert.throws(()=>launchCli(args,{config:{executable:process.execPath,args:Array(33).fill('x'),env:{}}}),TypeError)
  assert.throws(()=>launchCli(args,{config:{executable:process.execPath,args:['中'.repeat(12000)],env:{}}}),TypeError)
  assert.throws(()=>launchCli(args,{...options('valid'),timeoutMs:0}),TypeError)
  const result=await executeCli(args,{...options('valid'),timeoutMs:Number.MAX_SAFE_INTEGER})
  assert.equal(result.state,'completed')
})

test('spawn errors reject publication and settle the outcome without raw paths',async()=>{
  const handle=launchCli(args,{config:{executable:'missing-any-a2a-test-executable',env:{}}})
  const results=await Promise.allSettled([handle.spawned,handle.result])
  assert.ok(results.every(result=>result.status==='rejected' && safeError('spawn')(result.reason)))
  handle.stop()
  assert.throws(()=>launchCli(args,{...options('wait'),signal:AbortSignal.abort()}),safeError('aborted'))
})

test('abort, explicit stop and deadlines await actual native process exit',async()=>{
  const dir=await mkdtemp(join(tmpdir(),'a2a-transport-'))
  try {
    for(const mode of ['abort','stop','deadline']) {
      const pidFile=join(dir,mode+'.pid'),controller=new AbortController()
      const handle=launchCli(args,{...options('wait',pidFile),signal:controller.signal,timeoutMs:mode==='deadline'?300:5000})
      const observed=handle.result.catch(error=>error)
      try {
        await handle.spawned
        let pid
        for(let i=0;i<100;i++) {try{pid=Number(await readFile(pidFile,'utf8'));break}catch(error){if(error.code!=='ENOENT')throw error}await delay(10)}
        assert.ok(pid,'fixture initialized')
        if(mode==='abort')controller.abort()
        else if(mode==='stop'){handle.stop();handle.stop()}
        const error=await observed
        assert.ok(safeError(mode==='deadline'?'timeout':'aborted')(error))
        assert.throws(()=>process.kill(pid,0),{code:'ESRCH'})
      }finally{handle.stop();await observed}
    }
  }finally{await rm(dir,{recursive:true,force:true})}
})
