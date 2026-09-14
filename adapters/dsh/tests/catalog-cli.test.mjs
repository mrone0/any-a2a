import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { createServer } from 'node:http'
import { execFileSync } from 'node:child_process'
import { createProvider } from '../src/index.js'
import { mountCapabilities } from '../src/capabilities.js'

const executable = process.env.ANY_A2A_TEST_BINARY || resolve('target/debug/any-a2a' + (process.platform === 'win32' ? '.exe' : ''))
test('client-owned CLI uses catalog credentials and capabilities without desktop service', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'a2a client 中文 '))
  let calls = 0
  const server = createServer(async (req, res) => {
    calls++
    assert.equal(req.headers.authorization, 'Bearer fixture-only')
    let body = ''
    for await (const chunk of req) body += chunk
    const rpc = JSON.parse(body)
    const result = { kind: 'message', parts: [{kind:'text',text:'independent'}, {kind:'data',data:{ok:true}}] }
    res.setHeader('content-type', 'application/json')
    res.end(JSON.stringify({jsonrpc:'2.0',id:rpc.id,result}))
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  try {
    const card = {name:'specialist',description:'fixture',version:'1',protocolVersion:'0.3.0',url:`http://127.0.0.1:${server.address().port}/rpc`,capabilities:{},defaultInputModes:['text'],defaultOutputModes:['text'],skills:[]}
    await writeFile(join(directory, 'agent-cards.jsonl'), JSON.stringify({id:'saved',source:'manual',agentCard:card,auth:{bearerToken:'fixture-only'}})+'\n')
    const catalog = execFileSync(executable, ['catalog'], {env:{...process.env,ANY_A2A_DATA_DIR:directory},encoding:'utf8'})
    assert.ok(!catalog.includes('fixture-only'))
    const config = {agentId:'saved',dataDir:directory,executable,toolName:'specialist'}
    let assemble
    mountCapabilities({on(_event, handler) { assemble = handler }}, config)
    const assembly = {sections:[]}
    await assemble(assembly, {}, async () => {})
    assert.match(assembly.sections[0].text, /Capability data/)
    const run = await createProvider(config).start({prompt:[{type:'text',text:'hello'}],signal:new AbortController().signal})
    const result = await run.result
    assert.equal(result.stopReason, 'completed')
    assert.equal(JSON.parse(result.output[0].text).parts[1].data.ok, true)
    await run.dispose()
    assert.equal(calls, 1)
    await writeFile(join(directory, 'agent-cards.jsonl'), JSON.stringify({id:'saved',deleted:true})+'\n')
    const missing = await createProvider(config).start({prompt:[{type:'text',text:'hello'}],signal:new AbortController().signal})
    assert.equal((await missing.result).stopReason, 'error')
    await missing.dispose()
    assert.equal(calls, 1)
  } finally {
    await new Promise(resolve => server.close(resolve))
    await rm(directory, {recursive:true,force:true})
  }
})
