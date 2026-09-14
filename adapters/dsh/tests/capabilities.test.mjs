import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mountCapabilities } from '../src/capabilities.js'
test('capabilities refresh every assembly and removed cards are not advertised', async () => {
  let cards = [{id:'one',info:{name:'First',description:'old',skills:[]}}]
  const server = createServer((req,res) => { res.setHeader('content-type','application/json'); res.end(JSON.stringify(cards)) })
  await new Promise(r=>server.listen(0,'127.0.0.1',r))
  try {
    let listener
    mountCapabilities({on(event,callback){assert.equal(event,'system-prompt/assemble'); listener=callback}}, {serviceUrl:`http://127.0.0.1:${server.address().port}`,agentId:'one',toolName:'a2a_one'})
    async function assemble(){const assembly={sections:[]}; await listener(assembly,{},async()=>assembly);return assembly.sections[0].text}
    assert.match(await assemble(), /old/)
    cards[0].info.description='updated skills'
    assert.match(await assemble(), /updated skills/)
    cards=[]
    assert.match(await assemble(), /no longer/)
  } finally { server.closeAllConnections(); await new Promise(r=>server.close(r)) }
})
