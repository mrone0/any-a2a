import {test} from 'node:test'
import assert from 'node:assert/strict'
import {AnyA2APlugin} from '../index.js'
test('registers native subagent without replacing task; denies primary and unowned execution',async()=>{
 const plugin=await AnyA2APlugin({client:{session:{get:async()=>({data:{}})}}})
 const config={};await plugin.config(config)
 assert.equal(config.agent['any-a2a-remote'].mode,'subagent')
 assert.equal(config.agent['any-a2a-remote'].permission['*'],'deny')
 assert.equal(plugin.tool.task,undefined)
 await assert.rejects(plugin.config(config),/overwrite/)
 await assert.rejects(plugin.tool.a2a_run.execute({},{agent:'build'}),/native A2A subagent/)
 await assert.rejects(plugin.tool.a2a_run.execute({},{agent:'any-a2a-remote',sessionID:'s'}),/parent-linked/)
})
