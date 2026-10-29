import { connect, call, upload } from './siyuan-api.mjs'
import { mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { execFileSync } from 'node:child_process'

/** 问思源：块级与行内 IAL 各自能活到哪一步。 */
const workspace = process.argv[2]
const port = Number(process.argv[3])
const WORK = 'D:/dev/session-sy/.tmp-inline-ial'
const stamp = new Date().toISOString().replace(/[-:TZ.]/gu, '').slice(0, 14)

const docId = '20260101000000-abcdefg'
const paraId = '20260101000000-abcdeff'
const documentNode = {
  ID: docId,
  Spec: '1',
  Type: 'NodeDocument',
  Properties: {},
  Children: [{
    ID: paraId,
    Type: 'NodeParagraph',
    Properties: { 'custom-block-src': '整段原文' },
    Children: [
      { Type: 'NodeText', Data: '正文', Properties: { 'custom-inline-src': '**正文**' } },
      { Type: 'NodeStrong', Properties: { 'custom-strong-src': '**加粗**' }, Children: [{ Type: 'NodeText', Data: '加粗' }] },
    ],
  }],
}

const target = await connect(workspace, port)
let notebookId
const report = { steps: [] }
try {
  const created = await call(target, '/api/notebook/createNotebook', { name: '行内属性探针 ' + stamp })
  notebookId = created?.notebook?.id
  if (!notebookId) throw new Error('createNotebook 没返回 id')

  rmSync(WORK, { recursive: true, force: true })
  mkdirSync(join(WORK, 'parts'), { recursive: true })
  writeFileSync(join(WORK, 'parts', docId + '.sy'), JSON.stringify(documentNode), 'utf8')
  const zipPath = join(WORK, 'probe.zip')
  execFileSync('powershell', ['-NoProfile', '-Command',
    "Compress-Archive -Path '" + join(WORK, 'parts') + "' -DestinationPath '" + zipPath + "' -Force",
  ], { encoding: 'utf8' })

  await upload(target, '/api/import/importSY', {
    file: { kind: 'file', filename: 'probe.zip', bytes: readFileSync(zipPath) },
    notebook: notebookId,
    toPath: '/',
  })

  let realDocId
  for (let attempt = 0; attempt < 40; attempt++) {
    try { await call(target, '/api/sqlite/flushTransaction') } catch { /* 未就绪 */ }
    const listed = await call(target, '/api/filetree/listDocsByPath', { notebook: notebookId, path: '/' })
    if ((listed?.files ?? []).length > 0) { realDocId = listed.files[0].id; break }
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  if (!realDocId) throw new Error('导入后笔记本里没有文档')

  const kramdown = await call(target, '/api/block/getBlockKramdown', { id: realDocId })
  report.kramdown = kramdown?.kramdown ?? kramdown
  const rows = await call(target, '/api/query/sql', {
    stmt: "SELECT id, type, ial FROM blocks WHERE root_id = '" + realDocId + "'",
  })
  report.blocks = rows
  report.hasInline = String(report.kramdown).includes('custom-inline-src')
  report.hasStrong = String(report.kramdown).includes('custom-strong-src')
  report.hasBlock = String(report.kramdown).includes('custom-block-src')
  console.log(JSON.stringify(report, null, 2))
} finally {
  if (notebookId) {
    await call(target, '/api/notebook/removeNotebook', { notebook: notebookId }).catch(() => {})
  }
  rmSync(WORK, { recursive: true, force: true })
}
