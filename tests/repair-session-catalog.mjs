// Offline, narrowly scoped migration. Stop every DSH writer before running.
// node --import tsx/esm <this-file> <DSH_ROOT> <SESSIONS_ROOT>
import fs from 'node:fs/promises'
import path from 'node:path'
import { pathToFileURL } from 'node:url'
import assert from 'node:assert/strict'
const [root, sessions] = process.argv.slice(2)
if (!root || !sessions) throw Error('DSH_ROOT and SESSIONS_ROOT required')
const {scanZstdFrames, decompressZstdFrame, compressZstdFrame} = await import(pathToFileURL(path.join(root,'packages/session/session-persistence-jsonl/src/zstd.ts')).href)
const stamp = Date.now()
let count = 0
async function walk(dir) {
  for (const entry of await fs.readdir(dir,{withFileTypes:true})) {
    const file=path.join(dir,entry.name)
    if (entry.isDirectory()) {await walk(file);continue}
    if (entry.name !== 'session.jsonl.zstd') continue
    const original=await fs.readFile(file), scan=scanZstdFrames(original)
    if (scan.tornStart !== undefined) {console.log('SKIP incomplete log',file);continue}
    const frames=[]
    let changes=0
    for (const range of scan.frames) {
      const encoded=original.subarray(range.start,range.end)
      const plain=(await decompressZstdFrame(encoded)).toString('utf8')
      let changed=false
      const rewritten=plain.split('\n').map(line=>{
        if(!line.trim())return line
        const event=JSON.parse(line)
        if(event.type === 'assistant/message' && event.data?.message?.source?.kind === 'plugin' && event.data.message.source.plugin === 'any-a2a') {
          const before=structuredClone(event)
          event.type='user/message'
          event.data={...event.data.message,role:'user'}
          assert.deepEqual(event.data.content,before.data.message.content)
          assert.equal(event.seq,before.seq)
          changed=true;changes++
          return JSON.stringify(event)
        }
        if(event.type !== 'subagent/catalog' || event.ignorable === true)return line
        // Only migrate the adapter's known redundant parent index records.
        if(event.data?.version !== 0 || event.data?.mode !== 'one-shot' || typeof event.data?.childId !== 'string')throw Error('Unexpected catalog record: '+file)
        event.ignorable=true;changed=true;changes++
        const output=JSON.stringify(event)
        const check=JSON.parse(output);delete check.ignorable
        assert.deepEqual(check,JSON.parse(line))
        return output
      }).join('\n')
      frames.push(changed ? await compressZstdFrame(rewritten) : encoded)
    }
    if(!changes)continue
    const replacement=Buffer.concat(frames)
    const verified=scanZstdFrames(replacement)
    assert.equal(verified.tornStart,undefined)
    assert.equal(verified.frames.length,scan.frames.length)
    for(const f of verified.frames)await decompressZstdFrame(replacement.subarray(f.start,f.end))
    const backup=file+'.backup-catalog-'+stamp
    await fs.copyFile(file,backup,1)
    assert.ok((await fs.readFile(file)).equals(original),'Concurrent modification')
    const temp=file+'.repair-'+stamp
    await fs.writeFile(temp,replacement,{flag:'wx'})
    const handle=await fs.open(temp,'r+');await handle.sync();await handle.close()
    assert.ok((await fs.readFile(file)).equals(original),'Concurrent modification')
    await fs.rename(temp,file)
    count++;console.log(JSON.stringify({file,changes,backup}))
  }
}
await walk(sessions)
console.log('Repaired files:',count)
