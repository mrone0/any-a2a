/** DSH-native one-shot delegation; omitted background flag means background. */
import { defineTool } from '@deepseek-ai/dsh-tools'
import { openRemoteSession } from './remote-session.js'
export const name = 'any-a2a-delegation-tool'
export const inject = ['tools', 'subagents', 'jobs', 'sessions', 'sessionProjections']

export function apply(ctx, config) {
  if (!config?.provider || !config?.toolName) throw Error('provider and toolName required')
  const settle = async start => {
    const run = await start
    try { return await run.result } finally { await run.dispose() }
  }
  ctx.effect(() => ctx.tools.register(defineTool({
    name: config.toolName,
    description: `Delegate to remote subagent ${config.toolName}. Runs in background by default and immediately returns a parent-owned job. Continue your own independent work, then collect using job_output. Set run_in_background=false only when you need to wait now. This is native subagent delegation, not a resumable local session.`,
    parameters: {
      description: {type:'string', required:true, description:'Short task label'},
      prompt: {type:'string', required:true, description:'Complete standalone task for the remote subagent'},
      run_in_background: {type:'boolean', description:'Defaults to true; false explicitly waits for completion'},
    },
    output: {
      schema: {type:'object', additionalProperties:false, properties:{kind:{type:'string',required:true},jobId:{type:'string'},sessionId:{type:'string'},output:{type:'array',items:{type:'json'}},stopReason:{type:'string'}}},
      render: (_args, value) => [{type:'text',text:value.kind === 'background'
        ? `Delegated to ${config.toolName}; background subagent job ${value.jobId}, child session ${value.sessionId}. Continue independent work, then collect with job_output.`
        : JSON.stringify(value)}],
    },
    isConcurrencySafe: () => true,
    async execute(args, exec) {
      if (!exec.agent) throw Error('Subagent delegation requires parent Agent')
      exec.signal.throwIfAborted()
      if (!ctx.subagents.getProvider(config.provider)) throw Error('Remote subagent provider unavailable')
      const request = {label:args.description,parent:exec.agent,prompt:[{type:'text',text:args.prompt}]}
      const transcript = openRemoteSession(ctx, exec.agent, `${config.toolName}: ${args.description}`, request.prompt, config.provider)
      const settleRecorded = async (start) => {
        try {
          const result = await settle(start)
          transcript.close(result)
          return result
        } catch (error) {
          transcript.close({output:[],stopReason:'error'})
          throw error
        }
      }
      if (args.run_in_background === false) {
        const result = await settleRecorded(ctx.subagents.start(config.provider,{...request,signal:exec.signal}))
        return {kind:'foreground',sessionId:transcript.id,output:result.output,stopReason:result.stopReason}
      }
      let jobId
      try { jobId = ctx.jobs.start({
        kind:'subagent',label:`${config.toolName}: ${args.description}`,owner:exec.agent,
        run: () => {
          const controller = new AbortController()
          return {
            cancel: () => controller.abort(),
            done: settleRecorded(ctx.subagents.start(config.provider,{...request,signal:controller.signal})),
          }
        },
      }) } catch (error) { transcript.close({output:[],stopReason:'error'}); throw error }
      return {kind:'background',jobId,sessionId:transcript.id}
    },
  })))
}
