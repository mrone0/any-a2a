import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { setTimeout as delay } from 'node:timers/promises'
import { createProvider } from '../src/index.js'

const fixture=fileURLToPath(new URL('../../common/tests/fixture-cli.mjs',import.meta.url))
const provider=(mode,pidFile)=>createProvider({cardUrl:'https://example.test/card',executable:process.execPath,args:[fixture,mode,pidFile || '-']})
const request=signal=>({prompt:[{type:'text',text:'fixture'}],signal})

test('DSH CLI publication returns before completion; disposal awaits native close',async()=>{
  const dir=await mkdtemp(join(tmpdir(),'a2a-dsh-publication-'))
  let run
  try {
    const pidFile=join(dir,'child.pid')
    run=await provider('wait',pidFile).start(request(new AbortController().signal))
    let settled=false
    run.result.then(()=>settled=true)
    let pid
    for(let i=0;i<100;i++){try{pid=Number(await readFile(pidFile,'utf8'));break}catch(error){if(error.code!=='ENOENT')throw error}await delay(10)}
    assert.ok(pid)
    assert.equal(settled,false)
    await run.dispose()
    assert.equal((await run.result).stopReason,'aborted')
    assert.throws(()=>process.kill(pid,0),{code:'ESRCH'})
    await run.dispose()
  }finally{await run?.dispose();await rm(dir,{recursive:true,force:true})}
})

test('prepublication cancellation rejects start instead of publishing an aborted run',async()=>{
  const controller=new AbortController()
  const started=provider('wait').start(request(controller.signal))
  controller.abort()
  await assert.rejects(started,{name:'AbortError'})
  assert.throws(()=>createProvider({cardUrl:'https://example.test/card',args:Array(33).fill('x')}),/32 arguments/)
  assert.throws(()=>createProvider({cardUrl:'https://example.test/card',args:['中'.repeat(12000)]}),/32 KiB/)
})

test('postpublication malformed output and exits become sanitized DSH outcomes',async()=>{
  for(const mode of ['invalid','nonzero']) {
    const run=await provider(mode).start(request(new AbortController().signal))
    try {
      const result=await run.result
      assert.equal(result.stopReason,'error')
      assert.deepEqual(result.output,[])
      assert.doesNotMatch(result.diagnostic,/secret-token|private-prompt/)
    }finally{await run.dispose()}
  }
})
