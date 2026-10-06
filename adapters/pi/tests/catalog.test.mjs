import { test } from 'node:test'
import assert from 'node:assert/strict'
import { capabilityPrompt } from '../capabilities.mjs'

test('saved and uncached URL metadata have distinct truthful capability hints', () => {
  const prompt = capabilityPrompt([
    {id:'old-url',source:'url',info:null,raw:null},
    {id:'cached-url',source:'url',info:{name:'Known specialist',description:'Last saved description',skills:[]}},
  ])
  assert.match(prompt, /not live availability checks/)
  assert.match(prompt, /"id":"old-url","name":"old-url","metadataStatus":"unavailable"/)
  assert.match(prompt, /"metadataStatus":"cached"/)
  assert.match(prompt, /Last saved description/)
})
