import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { randomUUID } from 'node:crypto'
import { RunLifecycle, RunPersistenceError } from '../lifecycle.mjs'
import { RunStore } from '../runs.mjs'
import { removeFixtureDirectory } from './temp-dir.mjs'

const owner='fixture-parent'
const outcome={text:'result',raw:{kind:'message',parts:[{kind:'text',text:'result'}]},state:'completed',task_id:null,context_id:null}
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no});return {promise,resolve,reject}}
const waiting=(_record,signal)=>new Promise((_resolve,reject)=>{
  const abort=()=>reject(Error('Local delegation stopped'))
  signal.addEventListener('abort',abort,{once:true})
  if(signal.aborted)abort()
})
async function fixture(run) {
  const dir=await mkdtemp(join(tmpdir(),'pi-lifecycle-'))
  const manager=new RunLifecycle(),store=new RunStore(dir,owner)
  const options={owner,store,record:{agentId:'fixture',task:'standalone task'},launch:waiting}
  try {await run({manager,store,options,dir})}
  finally {await manager.shutdown();await removeFixtureDirectory(dir,'pi-lifecycle-')}
}

test('six parallel starts reserve four slots before preflight; failures release admission',async()=>{
  await fixture(async({manager,store,options})=>{
    const gate=deferred();let entered=0
    const starts=Array.from({length:6},()=>manager.start({...options,background:true,preflight:async()=>{entered++;await gate.promise}}))
    const all=Promise.allSettled(starts)
    assert.equal(manager.size,4)
    try {
      gate.resolve()
      const results=await all
      assert.equal(entered,4)
      assert.equal(results.filter(result=>result.status==='fulfilled').length,4)
      assert.equal(results.filter(result=>result.status==='rejected' && /At most 4/.test(result.reason.message)).length,2)
      assert.equal(await store.count(),4)
      await manager.shutdown()
      assert.equal(manager.size,0)
      assert.ok((await store.list()).every(record=>record.state==='stopped' && record.endedAt))
      manager.activate()
      const next=await manager.start({...options,launch:()=>Promise.resolve(outcome)})
      assert.equal((await next.result).state,'completed')
    }finally{gate.resolve()}
  })
})

test('pending retention claims and saved records together never exceed 200',async()=>{
  await fixture(async({manager,store,options})=>{
    for(let i=0;i<198;i++)await store.save({version:1,id:randomUUID(),owner,agentId:'old',task:'old',state:'completed',startedAt:i,endedAt:i+1,progress:[]})
    const results=await Promise.allSettled(Array.from({length:3},()=>manager.start({...options,background:true})))
    assert.equal(results.filter(result=>result.status==='fulfilled').length,2)
    assert.equal(results.filter(result=>result.status==='rejected' && /retention/.test(result.reason.message)).length,1)
    assert.equal(await store.count(),200)
    await manager.shutdown()
    assert.equal(await store.count(),200)
  })
})

test('initial updates, synchronous launch failure and prelaunch abort persist honest terminal states',async()=>{
  await fixture(async({manager,store,options})=>{
    let launches=0
    const controller=new AbortController()
    await assert.rejects(manager.start({...options,signal:controller.signal,onInitial:()=>controller.abort(),launch:()=>{launches++;return Promise.resolve(outcome)}}),/Local delegation stopped/)
    await assert.rejects(manager.start({...options,onInitial:()=>{throw Error('update failed')}}),/update failed/)
    await assert.rejects(manager.start({...options,launch:()=>{throw Error('synchronous launch failed')}}),/synchronous launch failed/)
    assert.equal(launches,0)
    const records=await store.list()
    assert.deepEqual(records.map(record=>record.state).sort(),['failed','failed','stopped'])
    assert.ok(records.every(record=>record.endedAt && record.updatedAt===record.endedAt))
    assert.equal(manager.size,0)
    await assert.rejects(manager.start({...options,signal:AbortSignal.abort()}),/Local delegation stopped/)
    assert.equal(await store.count(),3,'pre-aborted admission creates no record')
  })
})

test('shutdown waits pending preflight, blocks launches and resumes on activation',async()=>{
  await fixture(async({manager,store,options})=>{
    const gate=deferred(),entered=deferred();let launches=0,shutdown
    const started=manager.start({...options,preflight:async()=>{entered.resolve();await gate.promise},launch:()=>{launches++;return Promise.resolve(outcome)}})
    const observed=started.catch(error=>error)
    await entered.promise
    let finished=false
    shutdown=manager.shutdown().then(()=>{finished=true})
    try {
      await Promise.resolve()
      assert.equal(finished,false)
      await assert.rejects(manager.start(options),/shutting down/)
      assert.throws(()=>manager.activate(),/not finished/)
      gate.resolve()
      assert.match((await observed).message,/Local delegation stopped/)
      await shutdown
      assert.equal(launches,0)
      assert.equal(await store.count(),0)
      manager.activate()
      const next=await manager.start({...options,launch:()=>Promise.resolve(outcome)})
      await next.result
    }finally{gate.resolve();await shutdown}
  })
})

test('abort during the initial save becomes stopped after that save finishes',async()=>{
  await fixture(async({manager,store,options})=>{
    const gate=deferred(),saved=deferred(),controller=new AbortController();let writes=0,launches=0
    const delayed={count:()=>store.count(),async save(record){await store.save(record);if(++writes===1){saved.resolve();await gate.promise}}}
    const started=manager.start({...options,store:delayed,signal:controller.signal,launch:()=>{launches++;return Promise.resolve(outcome)}})
    const observed=started.catch(error=>error)
    try {
      await saved.promise
      controller.abort()
      gate.resolve()
      assert.match((await observed).message,/Local delegation stopped/)
      const [record]=await store.list()
      assert.equal(record.state,'stopped')
      assert.ok(record.endedAt)
      assert.equal(launches,0)
      assert.equal(manager.size,0)
    }finally{gate.resolve()}
  })
})

test('background cancellation remains linked until explicit publication',async()=>{
  await fixture(async({manager,store,options})=>{
    const before=new AbortController()
    const unpublished=await manager.start({...options,background:true,signal:before.signal})
    before.abort()
    await assert.rejects(unpublished.result,/Local delegation stopped/)
    assert.equal((await store.get(unpublished.id)).state,'stopped')
    const controller=new AbortController(),completion=deferred()
    const published=await manager.start({...options,background:true,signal:controller.signal,launch:()=>completion.promise})
    published.publish()
    controller.abort()
    assert.equal(published.controller.signal.aborted,false)
    completion.resolve(outcome)
    await published.result
    assert.equal((await store.get(published.id)).state,'completed')
  })
})

test('ownership checks precede inspect/stop; terminal delivery errors are attempted once',async()=>{
  await fixture(async({manager,store,options})=>{
    const entry=await manager.start({...options,background:true})
    assert.throws(()=>manager.get(entry.id,'other-parent'),/owner mismatch/)
    await assert.rejects(manager.stop(entry.id,'other-parent'),/owner mismatch/)
    assert.equal(entry.controller.signal.aborted,false)
    await manager.stop(entry.id,owner)
    let notifications=0
    const completed=await manager.start({...options,launch:()=>Promise.resolve(outcome),onTerminal:()=>{notifications++;throw Error('delivery failed')}})
    await assert.rejects(completed.result,/delivery failed/)
    assert.equal((await store.get(completed.id)).state,'completed')
    assert.equal(manager.size,0)
    assert.equal(notifications,1)
    await manager.shutdown()
    manager.activate()
    assert.equal(notifications,1)
  })
})

test('persistence failure is explicit, releases slots and never resubmits remote work',async()=>{
  await fixture(async({manager,store,options})=>{
    let launches=0
    await assert.rejects(manager.start({...options,store:{count:async()=>0,save:async()=>{throw Error('disk failed')}},launch:()=>{launches++;return Promise.resolve(outcome)}}),error=>error instanceof RunPersistenceError && error.phase==='initial')
    assert.equal(launches,0)
    assert.equal(manager.size,0)
    assert.equal(await store.count(),0)
    let writes=0
    const faulty={count:()=>store.count(),async save(record){if(++writes===2)throw Error('disk failed');await store.save(record)}}
    const entry=await manager.start({...options,store:faulty,launch:()=>{launches++;return Promise.resolve(outcome)}})
    await assert.rejects(entry.result,error=>error instanceof RunPersistenceError && error.phase==='terminal' && error.record.state==='completed')
    assert.equal(launches,1)
    assert.equal(writes,2,'no automatic persistence or transport retry')
    assert.equal(manager.size,0)
    assert.equal((await store.get(entry.id)).state,'running','saved snapshot may be stale; failure is reported')
  })
})

test('record counting does not parse historical large or malformed result bodies',async()=>{
  await fixture(async({store})=>{
    await mkdir(store.dir,{recursive:true})
    await writeFile(store.path(randomUUID()),'not JSON')
    assert.equal(await store.count(),1)
    await assert.rejects(store.list())
  })
})
