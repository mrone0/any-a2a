// Source-only DSH test bootstrap. No build, links, or installs in the clone.
import { register } from 'node:module';
import { readFileSync, statSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { resolve as resolvePath } from 'node:path';
import { execFileSync } from 'node:child_process';
import { isMainThread } from 'node:worker_threads';
import { parse } from 'jsonc-parser';

const root = fileURLToPath(new URL('../../../.research/deepseek-harness/', import.meta.url));
const adapter = new URL('../package.json', import.meta.url).href;
const pin = 'c291e7961a515f6d7af9304e7fd1d257929aef26';
const paths = parse(readFileSync(resolvePath(root, 'tsconfig.base.json'), 'utf8')).compilerOptions.paths;

if (isMainThread) {
  const actual = execFileSync('git', ['-C', root, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  if (actual !== pin) throw new Error(`DSH integration requires ${pin}; found ${actual}`);
  // tsx otherwise caches transformed sources under the system temporary directory.
  process.env.TSX_DISABLE_CACHE = '1';
  await import('tsx/esm');
  register(import.meta.url);
}

export async function resolve(specifier, context, nextResolve) {
  const targets = paths[specifier];
  if (targets) {
    for (const target of targets) {
      const base = resolvePath(root, target);
      for (const candidate of [base, `${base}.ts`, resolvePath(base, 'index.ts')]) {
        try {
          if (statSync(candidate).isFile()) {
            return nextResolve(pathToFileURL(candidate).href, context);
          }
        } catch (error) {
          if (error.code !== 'ENOENT' && error.code !== 'ENOTDIR') throw error;
        }
      }
    }
    throw new Error(`No pinned DSH source for ${specifier}`);
  }
  if (specifier.startsWith('@deepseek-ai/')) {
    throw new Error(`Missing source alias for ${specifier}; refusing published/built fallback`);
  }
  // External dependencies of cloned source resolve only from this adapter.
  if (!specifier.startsWith('.') && !specifier.startsWith('/') && !specifier.includes(':')) {
    return nextResolve(specifier, { ...context, parentURL: adapter });
  }
  return nextResolve(specifier, context);
}
