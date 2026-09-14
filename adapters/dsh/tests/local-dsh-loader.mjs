// Resolve installed DSH workspace peers for local, non-pinned smoke tests.
// Usage: DSH_TEST_ROOT=... node --import ./.../local-dsh-loader.mjs --test ...
import { registerHooks } from 'node:module'
import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
const root = process.env.DSH_TEST_ROOT
if (!root) throw Error('DSH_TEST_ROOT required')
const packages = new Map()
for (const base of [join(root,'vendor'), ...readdirSync(join(root,'packages')).filter(n=>!n.includes('.')).map(n=>join(root,'packages',n))]) {
  if (!existsSync(base)) continue
  for (const entry of readdirSync(base,{withFileTypes:true})) {
    if (!entry.isDirectory()) continue
    const file=join(base,entry.name,'package.json')
    if (!existsSync(file)) continue
    const pkg=JSON.parse(readFileSync(file,'utf8'))
    const main=join(base,entry.name,'lib/index.js')
    if (existsSync(main)) packages.set(pkg.name,pathToFileURL(main).href)
  }
}
registerHooks({resolve(specifier,context,next){
  return next(packages.get(specifier)||specifier,context)
}})
