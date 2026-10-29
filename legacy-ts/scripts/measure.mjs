import { readFileSync } from 'node:fs'
import { zstdDecompressSync } from 'node:zlib'
import { StringDecoder } from 'node:string_decoder'

const buf = readFileSync(process.argv[2])
const starts = []
for (let i = 0; i + 4 <= buf.length; i++) {
  if (buf[i] === 0x28 && buf[i+1] === 0xb5 && buf[i+2] === 0x2f && buf[i+3] === 0xfd) starts.push(i)
}
const decoder = new StringDecoder('utf8')
let carry = ''
let lines = 0
let bytes = 0
const types = new Map()
for (let k = 0; k < starts.length; k++) {
  const end = k + 1 < starts.length ? starts[k+1] : buf.length
  const text = carry + decoder.write(zstdDecompressSync(buf.subarray(starts[k], end)))
  const parts = text.split('\n')
  carry = parts.pop() ?? ''
  for (const line of parts) {
    if (line.trim().length === 0) continue
    lines++
    bytes += line.length
    if (lines > 1) {
      const m = /"type":"([^"]+)"/.exec(line.slice(0, 200))
      if (m) types.set(m[1], (types.get(m[1]) ?? 0) + line.length)
    }
  }
}
carry += decoder.end()
console.log(JSON.stringify({
  frames: starts.length,
  lines,
  decompressedMB: Math.round(bytes / 1048576),
  topTypes: [...types.entries()].sort((a,b) => b[1] - a[1]).slice(0, 6).map(([t,b]) => t + ': ' + Math.round(b/1048576) + 'MB')
}))
