/**
 * 端到端验收：把会话导入运行中的思源，然后只从思源那一侧取事实。
 *
 * 三件事分开量，互不代替：
 *   1. 我们写进 .sy 的块类型分布（事实 A）；
 *   2. 思源索引里留下的块类型分布（事实 B）—— 两者必须相等，否则就是被思源改了或丢了；
 *   3. 在思源里改一个段落块，再用 kramdown 读回来 —— 块必须真的可编辑。
 *
 * 我们自己的读写器不参与判断。默认在自己新建的临时笔记本里做，结束删掉；--keep 保留。
 */

import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { join } from 'node:path'

import { connect, call, upload } from './siyuan-api.mjs'

// 仓库根：本脚本在 scripts/ 下，往上一级
const here = join(import.meta.dirname, '..')
// 引擎二进制在兄弟仓库 rebec 里（rebec 是 Rust 包，产物是 exe / .node / rlib）
const BINARY = join(here, '..', 'rebec', 'target', 'release', 'rebec.exe')
const WORK = join(here, '.tmp-siyuan')

/** 思源块表里的 type 代码。 */
const TYPE_CODE = {
  NodeDocument: 'd', NodeParagraph: 'p', NodeHeading: 'h', NodeBlockquote: 'b', NodeCallout: 'callout',
  NodeCodeBlock: 'c', NodeList: 'l', NodeListItem: 'i', NodeTable: 't',
  NodeSuperBlock: 's', NodeMathBlock: 'm', NodeThematicBreak: 'tb',
}

/** 递归统计一份 .sy 里的块类型。行内节点不计。 */
function countBlocks(node, into) {
  const type = node?.Type
  const isBlock = typeof type === 'string' && Object.prototype.hasOwnProperty.call(TYPE_CODE, type)
  if (isBlock) into[type] = (into[type] ?? 0) + 1
  for (const child of node?.Children ?? []) countBlocks(child, into)
  return into
}

const argv = process.argv.slice(2).filter(item => item !== '--keep')
const keep = process.argv.slice(2).includes('--keep')
const [session, workspace, portText] = argv
if (!session || !workspace) {
  console.error('用法: node scripts/siyuan-import.mjs <session.jsonl.zstd> <workspace> <port> [--keep]')
  process.exit(2)
}
const port = Number(portText)
if (!Number.isInteger(port) || port <= 0) {
  console.error('缺少内核端口')
  process.exit(2)
}

const target = await connect(workspace, port)
const report = { steps: [] }
const notebookName = 'dsh-session-sy 验证 ' + new Date().toISOString().replace(/[:.]/g, '-')
let notebookId

try {
  const created = await call(target, '/api/notebook/createNotebook', { name: notebookName })
  notebookId = created?.notebook?.id
  if (!notebookId) throw new Error('createNotebook 没返回 id')
  report.steps.push({ step: 'createNotebook', notebookId })

  // 1. 导出分片，并统计我们写进去的块
  rmSync(WORK, { recursive: true, force: true })
  const partsDir = join(WORK, 'parts')
  mkdirSync(partsDir, { recursive: true })
  const emitted = JSON.parse(execFileSync(BINARY, ['emit', session, partsDir], { encoding: 'utf8' }).trim().split('\n').pop())
  const written = {}
  for (const name of readdirSync(partsDir)) {
    if (!name.endsWith('.sy')) continue
    countBlocks(JSON.parse(readFileSync(join(partsDir, name), 'utf8')), written)
  }
  report.steps.push({ step: 'emit', events: emitted.events, parts: emitted.parts, written })

  // 2. 导入
  const zipPath = join(here, '.tmp-import.zip')
  rmSync(zipPath, { force: true })
  execFileSync('powershell', ['-NoProfile', '-Command',
    'Compress-Archive -Path \'' + partsDir + '\' -DestinationPath \'' + zipPath + '\' -Force',
  ], { encoding: 'utf8' })
  await upload(target, '/api/import/importSY', {
    file: { kind: 'file', filename: 'parts.zip', bytes: readFileSync(zipPath) },
    notebook: notebookId,
    toPath: '/',
  })
  rmSync(zipPath, { force: true })

  // 3. 思源那一侧：实际文档 id
  // 列举文档要连续两次稳定才算数：导入过程中会短暂多出/少掉条目。
  let docIds = []
  let stable = 0
  for (let attempt = 0; attempt < 60; attempt++) {
    try { await call(target, '/api/sqlite/flushTransaction') } catch { }
    const listed = await call(target, '/api/filetree/listDocsByPath', { notebook: notebookId, path: '/' })
    const next = (listed?.files ?? []).map(item => item.id)
    if (next.length === docIds.length) stable += 1
    else stable = 0
    docIds = next
    if (docIds.length === emitted.parts && stable >= 2) break
    await new Promise(resolve => setTimeout(resolve, 300))
  }
  report.steps.push({ step: 'imported', documents: docIds.length, expectedParts: emitted.parts })
  if (docIds.length !== emitted.parts) throw new Error('导入的文档数与分片数不一致')

  // 4. 思源索引里的块类型分布
  const actual = {}
  for (let attempt = 0; attempt < 40; attempt++) {
    for (const key of Object.keys(actual)) delete actual[key]
    try { await call(target, '/api/sqlite/flushTransaction') } catch { }
    for (const id of docIds) {
      const rows = await call(target, '/api/query/sql', {
        stmt: "SELECT type, COUNT(*) AS n FROM blocks WHERE root_id = '" + id + "' GROUP BY type",
      })
      for (const row of rows ?? []) {
        const name = Object.entries(TYPE_CODE).find(([, code]) => code === row.type)?.[0] ?? row.type
        actual[name] = (actual[name] ?? 0) + Number(row.n)
      }
    }
    const total = Object.values(actual).reduce((sum, n) => sum + n, 0)
    const expected = Object.values(written).reduce((sum, n) => sum + n, 0)
    if (total >= expected) break
    await new Promise(resolve => setTimeout(resolve, 300))
  }
  report.steps.push({ step: 'siyuanBlocks', actual })

  const diff = {}
  for (const key of new Set([...Object.keys(written), ...Object.keys(actual)])) {
    diff[key] = (actual[key] ?? 0) - (written[key] ?? 0)
  }
  report.diff = diff
  report.blocksMatch = Object.values(diff).every(value => value === 0)

  // 5. 编辑：改一个段落块
  const paragraphRows = await call(target, '/api/query/sql', {
    stmt: "SELECT id FROM blocks WHERE root_id = '" + docIds[0] + "' AND type = 'p' LIMIT 1",
  })
  const blockId = paragraphRows?.[0]?.id
  if (!blockId) throw new Error('找不到可编辑的段落块')
  const before = await call(target, '/api/block/getBlockKramdown', { id: blockId })
  const marker = '【编辑测试 ' + new Date().toISOString() + '】'
  await call(target, '/api/block/updateBlock', { id: blockId, dataType: 'markdown', data: marker })
  const after = await call(target, '/api/block/getBlockKramdown', { id: blockId })
  const afterText = String(after.kramdown ?? '').trim()
  report.edited = { blockId, before: String(before.kramdown ?? '').slice(0, 50), after: afterText.slice(0, 60) }

  report.ok = report.blocksMatch && afterText.includes('编辑测试')
} catch (error) {
  report.ok = false
  report.error = String(error && error.message ? error.message : error)
} finally {
  rmSync(WORK, { recursive: true, force: true })
  if (notebookId && keep) {
    report.steps.push({ step: 'kept', notebookId, notebookName })
  } else if (notebookId) {
    try { await call(target, '/api/notebook/removeNotebook', { notebook: notebookId }) } catch { }
  }
}

console.log(JSON.stringify(report, null, 1))
if (report.ok !== true) process.exitCode = 1
