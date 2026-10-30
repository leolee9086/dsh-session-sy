/**
 * 探针：思源到底保得住哪些块结构。
 *
 * 造一份已知内容的 .sy（引用块带/不带标记、围栏代码块、段落、标题），导入运行中的思源，
 * 再从内核的块表里读回来，逐类对比「写进去的」和「留下来的」。
 * 这一层没有我们自己的读写器参与 —— 判据只有思源。
 */

import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import { connect, call, upload } from './siyuan-api.mjs'

const here = join(import.meta.dirname, '..')
const WORK = join(here, '.tmp-format')

const workspace = process.argv[2]
const port = Number(process.argv[3])
if (!workspace || !Number.isInteger(port)) {
  console.error('用法: node scripts/siyuan-probe-format.mjs <workspace> <port>')
  process.exit(2)
}

/** 生成符合思源块 id 格式的 id。 */
let serial = 0
const stamp = new Date().toISOString().replace(/\D/g, '').slice(0, 14)
function nextId() {
  const tail = serial.toString(36).padStart(7, '0')
  serial += 1
  return stamp + '-' + tail
}

function textBlock(text) {
  const node = { Type: 'NodeText', Data: text }
  return node
}

function paragraph(text) {
  const id = nextId()
  return { ID: id, Type: 'NodeParagraph', Properties: { id }, Children: [textBlock(text)] }
}

function codeBlock(data) {
  const id = nextId()
  return {
    ID: id, Type: 'NodeCodeBlock', IsFencedCodeBlock: true,
    CodeBlockFenceChar: 96, CodeBlockFenceLen: 3,
    CodeBlockOpenFence: 'YGBg', CodeBlockInfo: '', CodeBlockCloseFence: 'YGBg',
    Properties: { id },
    Children: [
      { Type: 'NodeCodeBlockFenceOpenMarker', Data: '```', CodeBlockFenceLen: 3 },
      { Type: 'NodeCodeBlockFenceInfoMarker', CodeBlockInfo: '' },
      { Type: 'NodeCodeBlockCode', Data: data },
      { Type: 'NodeCodeBlockFenceCloseMarker', Data: '```', CodeBlockFenceLen: 3 },
    ],
  }
}

function callout(children, calloutType, title, icon) {
  const id = nextId()
  const node = {
    ID: id, Type: 'NodeCallout',
    CalloutType: calloutType, CalloutTitle: title, CalloutIcon: icon,
    Properties: { id },
    Children: children,
  }
  return node
}

function paragraphWithSoftBreak(lines) {
  const id = nextId()
  return { ID: id, Type: 'NodeParagraph', Properties: { id }, Children: [textBlock(lines.join('\n'))] }
}

function blockquote(children, withMarker) {
  const id = nextId()
  const items = []
  if (withMarker) items.push({ Type: 'NodeBlockquoteMarker', Data: '> ' })
  for (const child of children) items.push(child)
  return { ID: id, Type: 'NodeBlockquote', Properties: { id }, Children: items }
}

function heading(level, text) {
  const id = nextId()
  return {
    ID: id, Type: 'NodeHeading', HeadingLevel: level, Properties: { id },
    Children: [textBlock(text)],
  }
}

const docId = nextId()
const documentNode = {
  ID: docId, Spec: '2', Type: 'NodeDocument',
  Properties: { id: docId, title: '格式探针' },
  Children: [
    // 1. callout：自定义类型 + 三条段落（模拟一条多段消息）
    callout([
      paragraph('第一段'),
      paragraph('第二段'),
      paragraph('第三段'),
    ], 'user', '用户', '👤'),
    // 2. callout：内建 NOTE 类型
    callout([paragraph('内建类型 callout')], 'NOTE', 'Note', '✏️'),
    // 3. 段落内含软换行（单个换行）
    paragraphWithSoftBreak(['第一行', '第二行', '第三行']),
    // 4. 引用块（带标记）作为对照
    blockquote([paragraph('引用块内容')], true),
    // 5. 代码块 + 附录
    codeBlock('{"top":"level"}'),
    heading(2, '附录'),
    codeBlock('{"appendix":true}'),
  ],
}
const expected = {
  NodeDocument: 1,
  NodeCallout: 2,
  NodeParagraph: 6,
  NodeBlockquote: 1,
  NodeCodeBlock: 2,
  NodeHeading: 1,
}
const target = await connect(workspace, port)
const report = { docId, expected, steps: [] }
let notebookId

try {
  const created = await call(target, '/api/notebook/createNotebook', { name: '格式探针 ' + stamp })
  notebookId = created?.notebook?.id
  if (!notebookId) throw new Error('createNotebook 没返回 id')

  rmSync(WORK, { recursive: true, force: true })
  mkdirSync(join(WORK, 'parts'), { recursive: true })
  const syPath = join(WORK, 'parts', docId + '.sy')
  writeFileSync(syPath, JSON.stringify(documentNode), 'utf8')

  const zipPath = join(WORK, 'probe.zip')
  execFileSync('powershell', ['-NoProfile', '-Command',
    'Compress-Archive -Path \'' + join(WORK, 'parts') + '\' -DestinationPath \'' + zipPath + '\' -Force',
  ], { encoding: 'utf8' })

  await upload(target, '/api/import/importSY', {
    file: { kind: 'file', filename: 'probe.zip', bytes: readFileSync(zipPath) },
    notebook: notebookId,
    toPath: '/',
  })

  // 思源会重新生成文档 id，所以从笔记本里读实际 id。
  let realDocId
  for (let attempt = 0; attempt < 40; attempt++) {
    try { await call(target, '/api/sqlite/flushTransaction') } catch { }
    const listed = await call(target, '/api/filetree/listDocsByPath', { notebook: notebookId, path: '/' })
    if ((listed?.files ?? []).length > 0) {
      realDocId = listed.files[0].id
      break
    }
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  if (!realDocId) throw new Error('导入后笔记本里没有文档')
  report.realDocId = realDocId

  // 等索引稳定：块数连续两次相同为止
  let previous = -1
  let rows = []
  for (let attempt = 0; attempt < 40; attempt++) {
    try { await call(target, '/api/sqlite/flushTransaction') } catch { }
    rows = await call(target, '/api/query/sql', {
      stmt: "SELECT type, COUNT(*) AS n FROM blocks WHERE root_id = '" + realDocId + "' GROUP BY type",
    })
    const total = (rows ?? []).reduce((sum, row) => sum + Number(row.n), 0)
    if (total === previous && total > 0) break
    previous = total
    await new Promise(resolve => setTimeout(resolve, 250))
  }

  const actual = Object.fromEntries((rows ?? []).map(row => [
    { d: 'NodeDocument', b: 'NodeBlockquote', p: 'NodeParagraph', c: 'NodeCodeBlock', h: 'NodeHeading' }[row.type] ?? row.type,
    Number(row.n),
  ]))
  report.steps.push({ step: 'blocksByType', actual })

  // 正文读回来，看内容有没有被改写
  const markdown = await call(target, '/api/block/getBlockKramdown', { id: realDocId })
  report.steps.push({ step: 'kramdown', bytes: String(markdown.kramdown ?? '').length, text: String(markdown.kramdown ?? '').slice(0, 600) })

  report.diff = Object.fromEntries(Object.entries(expected).map(([key, value]) => [key, (actual[key] ?? 0) - value]))
  report.ok = Object.values(report.diff).every(value => value === 0)
} catch (error) {
  report.ok = false
  report.error = String(error && error.message ? error.message : error)
} finally {
  rmSync(WORK, { recursive: true, force: true })
  if (notebookId) {
    try { await call(target, '/api/notebook/removeNotebook', { notebook: notebookId }) } catch { }
  }
}

console.log(JSON.stringify(report, null, 1))
if (report.ok !== true) process.exitCode = 1
