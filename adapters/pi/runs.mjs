import { randomUUID } from 'node:crypto'
import { mkdir, readFile, writeFile, rename, readdir, stat, unlink } from 'node:fs/promises'
import { join } from 'node:path'
import { createHash } from 'node:crypto'

/** Adapter-owned records, never Pi session messages. Partitioned by parent identity. */
export class RunStore {
  constructor(root, owner) {
    if (!owner) throw Error('Parent session identity required')
    this.owner=owner
    this.dir=join(root,createHash('sha256').update(owner).digest('hex'))
  }
  path(id) {
    if (!/^[0-9a-f-]{36}$/.test(id)) throw Error('Invalid A2A run ID')
    return join(this.dir,id+'.json')
  }
  async save(record) {
    if(record.owner!==this.owner)throw Error('Run owner mismatch')
    const text=JSON.stringify(record)
    if(Buffer.byteLength(text)>10*1024*1024)throw Error('Run record exceeds 10 MiB')
    await mkdir(this.dir,{recursive:true,mode:0o700})
    const file=this.path(record.id),temp=file+'.'+randomUUID()+'.tmp'
    try {
      await writeFile(temp,text,{flag:'wx',mode:0o600})
      await rename(temp,file)
    } finally {await unlink(temp).catch(e=>{if(e.code!=='ENOENT')throw e})}
  }
  async get(id) {
    const file=this.path(id)
    if((await stat(file)).size>10*1024*1024)throw Error('Run record too large')
    const r=JSON.parse(await readFile(file,'utf8'))
    if(r.version!==1||r.id!==id||r.owner!==this.owner||!Array.isArray(r.progress))throw Error('Invalid run record')
    return r
  }
  async list() {
    let names
    try{names=await readdir(this.dir)}catch(e){if(e.code==='ENOENT')return [];throw e}
    if(names.filter(n=>n.endsWith('.json')).length>200)throw Error('Run retention limit exceeded; archive old run files')
    const records=[]
    for(const name of names.filter(n=>n.endsWith('.json')))records.push(await this.get(name.slice(0,-5)))
    return records.sort((a,b)=>b.startedAt-a.startedAt)
  }
}
