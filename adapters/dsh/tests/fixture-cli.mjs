#!/usr/bin/env node
import { writeFileSync } from 'node:fs'
const [command, cardFlag, card, messageFlag, message] = process.argv.slice(2)
if (command !== 'run' || !['--card', '--card-file'].includes(cardFlag) || messageFlag !== '--message' || !card) process.exit(91)
if (message.startsWith('wait:')) {
  writeFileSync(message.slice(5), String(process.pid))
  setInterval(() => {}, 1000)
} else if (message === 'nonzero') {
  process.stderr.write('secret-token private-prompt')
  process.exitCode = 7
} else if (message === 'invalid') {
  process.stdout.write('not JSON secret-token')
} else if (message === 'overflow') {
  process.stdout.write('x'.repeat(10000))
} else if (message === 'fields') {
  process.stdout.write(JSON.stringify({ text: 'oops', state: 'completed' }))
} else {
  const text = message === 'args' ? JSON.stringify({ cardFlag, card }) : message === 'env' ? JSON.stringify({ token: process.env.ANY_A2A_TOKEN, secret: process.env.TEST_SECRET }) : message
  process.stdout.write(JSON.stringify({ text, task_id: null, context_id: 'remote-context', state: message.startsWith('state:') ? message.slice(6) : 'completed' }))
}
