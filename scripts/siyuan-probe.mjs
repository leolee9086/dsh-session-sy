/** 探一下思源内核能不能连上，列出笔记本。 */

import { connect, call } from './siyuan-api.mjs'

const workspace = process.argv[2]
const port = Number(process.argv[3])
if (!workspace || !Number.isInteger(port)) {
  console.error('用法: node scripts/siyuan-probe.mjs <workspace> <port>')
  process.exit(2)
}

const target = await connect(workspace, port)
const data = await call(target, '/api/notebook/lsNotebooks')
const notebooks = data.notebooks ?? []
console.log(JSON.stringify({
  port: target.port,
  notebooks: notebooks.map(item => ({ id: item.id, name: item.name, closed: item.closed })),
}, null, 1))
