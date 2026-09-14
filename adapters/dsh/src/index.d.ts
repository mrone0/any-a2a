import type { Context } from '@deepseek-ai/cordis'
import type { SubagentProvider } from '@deepseek-ai/dsh-subagent'

/** Configuration validated by apply, including when no schema-aware loader is used. */
export interface Config {
  /** HTTP(S) agent card URL, without embedded userinfo. */
  cardUrl?: string
  /** Absolute cached raw Agent Card JSON path; mutually exclusive with cardUrl. */
  cardFile?: string
  serviceUrl?: string
  serviceToken?: string
  /** Saved catalog agent; without serviceUrl uses a client-owned CLI process. */
  agentId?: string
  /** Absolute local catalog directory; defaults to the CLI's platform data directory. */
  dataDir?: string
  /** Native delegation tool bound to this provider; enables live capability hints. */
  toolName?: string
  /** Local any-a2a service mode; mutually exclusive with cardUrl/cardFile. */
  /** Registry key; default any-a2a. */
  providerName?: string
  /** Executable path or PATH command; default any-a2a. Not a shell command. */
  executable?: string
  /** Maximum buffered stdout bytes; default 1048576. */
  maxOutputBytes?: number
}
export declare const name: 'subagent-any-a2a'
export declare const inject: string[]
/** Create a text-only, one-shot remote provider. */
export declare function createProvider(config: Config): SubagentProvider
/** Register a provider through the host's effect-scoped registry. */
export declare function apply(ctx: Context, config: Config): void
