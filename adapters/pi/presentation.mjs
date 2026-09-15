// Plain-text card uses Pi's supported tool output/onUpdate surfaces.
export function runCard({agentName,agentId,id,task,background,startedAt}, state, now = Date.now()) {
  const clean = value => String(value ?? '').replace(/[\x00-\x1f\x7f]/g,' ').slice(0,200)
  return `远端子 Agent · ${clean(agentName || agentId)}\n状态：${state} · 模式：${background ? '后台' : '前台'} · 用时：${Math.max(0,Math.floor((now-startedAt)/1000))} 秒\nRun ID：${id}\n任务：${clean(task)}\n（扩展管理的远端委派，非 Pi 本地子会话）`
}
