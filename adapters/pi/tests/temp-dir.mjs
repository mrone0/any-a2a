import assert from 'node:assert/strict'
import { realpath, rm } from 'node:fs/promises'
import { basename, dirname, relative, isAbsolute } from 'node:path'
import { tmpdir } from 'node:os'

export async function removeFixtureDirectory(directory,prefix) {
  const root=await realpath(tmpdir()),target=await realpath(directory)
  assert.equal(dirname(target),root,'recursive cleanup must be a direct child of the real temp directory')
  assert.ok(basename(target).startsWith(prefix),'recursive cleanup must keep the fixture prefix')
  await rm(target,{recursive:true,force:true})
}

export async function assertFixtureChild(root,child) {
  const rootPath=await realpath(root),childPath=await realpath(child),path=relative(rootPath,childPath)
  assert.ok(path && path!=='..' && !path.startsWith('..\\') && !path.startsWith('../') && !isAbsolute(path),'record directory must remain inside the isolated fixture directory')
}
