import { writeFileSync } from 'node:fs'
const [mode, pidFile] = process.argv.slice(2)
if (pidFile && pidFile !== '-') writeFileSync(pidFile, String(process.pid))
const value = {text:'中文🙂',raw:{kind:'message',parts:[{kind:'text',text:'中文🙂'}]},task_id:null,context_id:null,state:'completed'}
const progress = {event:'progress',data:{stage:'request',method:'SendMessage'}}
const line = data => JSON.stringify(data)+'\n'
const final = line({event:'result',data:value})
if (mode === 'wait') setInterval(() => {}, 1000)
else if (mode === 'delay-close') {
  process.stdout.write(line(progress)+final)
  setTimeout(() => {}, 200)
} else if (mode === 'nonzero') {
  process.stderr.write('secret-token private-prompt')
  process.exitCode=7
} else if (mode === 'invalid') process.stdout.write('secret-token private-prompt\n')
else if (mode === 'utf8') process.stdout.write(Buffer.concat([Buffer.from('{"event":"result","data":{"text":"'),Buffer.from([0xff]),Buffer.from('"}}\n')]))
else if (mode === 'duplicate') process.stdout.write(final+final)
else if (mode === 'after-result') process.stdout.write(final+line(progress))
else if (mode === 'incomplete') process.stdout.write(final.trimEnd())
else if (mode === 'fields') process.stdout.write(line({event:'result',data:{text:'oops',state:'completed'}}))
else if (mode === 'no-result') process.stdout.write(line(progress))
else if (mode === 'large-progress') process.stdout.write(line({event:'progress',data:{stage:'remote_status',text:'x'.repeat(17000)}})+final)
else if (mode === 'overflow') process.stdout.write(line(progress).repeat(100)+final)
else if (mode === 'plain') process.stdout.write(JSON.stringify(value))
else if (mode === 'split-unicode') {
  const bytes=Buffer.from(line(progress)+final)
  for (const byte of bytes) process.stdout.write(Buffer.from([byte]))
} else process.stdout.write(line({event:'progress',data:{stage:'progress_truncated'}})+final)
