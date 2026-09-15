import {test} from 'node:test'
import assert from 'node:assert/strict'
import {deliverCompletion} from '../delivery.mjs'
test('completion queues once, only for owning parent, without remote payload',async()=>{
 const record={id:'run',owner:'parent',background:true,state:'completed',outcome:{text:'secret output'}}
 const store={save:async()=>{}}
 const sent=[];const send=(...args)=>sent.push(args)
 assert.equal(await deliverCompletion(record,store,()=> 'other',send),false)
 assert.equal(await deliverCompletion(record,store,()=> 'parent',send),true)
 assert.equal(await deliverCompletion(record,store,()=> 'parent',send),false)
 assert.equal(sent.length,1);assert.equal(sent[0][1].deliverAs,'followUp');assert.equal(sent[0][1].triggerTurn,true)
 assert.ok(!JSON.stringify(sent).includes('secret output'))
})
test('session replaced during claim leaves result pending',async()=>{
 let owner='parent';const record={id:'run',owner,background:true,state:'failed'}
 await deliverCompletion(record,{save:async()=>{owner='other'}},()=>owner,()=>assert.fail('wrong parent'))
 assert.equal(record.delivery,undefined)
})
test('uncertain send is retained and not retried',async()=>{
 const record={id:'run',owner:'parent',background:true,state:'completed'};let count=0
 const send=()=>{count++;throw Error('stale')};const store={save:async()=>{}}
 await deliverCompletion(record,store,()=> 'parent',send)
 await deliverCompletion(record,store,()=> 'parent',send)
 assert.equal(count,1);assert.equal(record.delivery.state,'uncertain')
})
