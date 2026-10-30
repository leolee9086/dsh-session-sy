// 把 rebec 的 cdylib 拷成 .node，供 Node 侧 require。
import { copyFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const source = join(import.meta.dirname, '..', '..', 'rebec', 'target', 'release', 'rebec.dll');
const target = join(import.meta.dirname, '..', '..', 'rebec', 'rebec.node');

if (!existsSync(source)) {
  console.error('找不到 ' + source + '，先 cargo build --release');
  process.exit(1);
}
copyFileSync(source, target);
console.log('已写出 ' + target);
