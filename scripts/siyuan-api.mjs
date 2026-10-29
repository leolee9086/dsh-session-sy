/**
 * 思源内核 API 的最小客户端，只给验证脚本用。
 *
 * token 从工作区的 conf/conf.json 读，不从命令行传 —— 免得它出现在进程列表里。
 */

import { readFile } from 'node:fs/promises'
import { join } from 'node:path'

/**
 * 读工作区里的 API token。
 *
 * 端口不在配置里：思源启动时挑一个空闲端口，所以由调用方从内核进程的监听端口拿到后传入。
 */
export async function connect(workspace, port) {
  if (!Number.isInteger(port) || port <= 0) throw new Error('需要内核实际监听的端口')
  const raw = await readFile(join(workspace, 'conf', 'conf.json'), 'utf8')
  const conf = JSON.parse(raw)
  const token = conf?.api?.token
  if (typeof token !== 'string' || token.length === 0) throw new Error('工作区没有 api.token')
  return { workspace, token, port }
}

/** 调一个 JSON 接口。 */
export async function call(target, endpoint, payload = {}) {
  const response = await fetch('http://127.0.0.1:' + target.port + endpoint, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: 'Token ' + target.token,
    },
    body: JSON.stringify(payload),
  })
  const text = await response.text()
  let parsed
  try { parsed = JSON.parse(text) } catch { throw new Error(endpoint + ' 返回非 JSON: ' + text.slice(0, 200)) }
  if (parsed.code !== 0) throw new Error(endpoint + ' 失败: ' + (parsed.msg ?? '') + ' ' + text.slice(0, 300))
  return parsed.data
}

/** 上传一个文件到导入接口。 */
export async function upload(target, endpoint, fields) {
  const form = new FormData()
  for (const [name, value] of Object.entries(fields)) {
    if (value && typeof value === 'object' && value.kind === 'file') {
      form.append(name, new Blob([value.bytes]), value.filename)
      continue
    }
    form.append(name, String(value))
  }
  const response = await fetch('http://127.0.0.1:' + target.port + endpoint, {
    method: 'POST',
    headers: { Authorization: 'Token ' + target.token },
    body: form,
  })
  const text = await response.text()
  let parsed
  try { parsed = JSON.parse(text) } catch { throw new Error(endpoint + ' 返回非 JSON: ' + text.slice(0, 200)) }
  if (parsed.code !== 0) throw new Error(endpoint + ' 失败: ' + (parsed.msg ?? '') + ' ' + text.slice(0, 300))
  return parsed.data
}
