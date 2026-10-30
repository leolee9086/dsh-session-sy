// 把 rebec 的构建产物拷进本仓库的 bin/ 和 addon/。
//
// 为什么要入库：如果 package.json 指向 ../rebec/target/release/rebec.exe，
// 别人 clone 下来根本没有那个目录（target 不进版本库），装不上也跑不起来。
//
// REBEC_SOURCE 可以覆盖姐妹仓库位置；默认按「两个仓库并排」找。
import { copyFileSync, existsSync, mkdirSync } from 'node:fs';
import { join, resolve } from 'node:path';

const here = join(import.meta.dirname, '..');
const sourceRoot = resolve(process.env.REBEC_SOURCE ?? join(here, '..', 'rebec'));
const built = join(sourceRoot, 'target', 'release');

const pairs = [
  [join(built, 'rebec.exe'), join(here, 'bin', 'rebec.exe')],
  [join(built, 'rebec.dll'), join(here, 'addon', 'rebec.node')],
];

mkdirSync(join(here, 'bin'), { recursive: true });
mkdirSync(join(here, 'addon'), { recursive: true });

for (const [from, to] of pairs) {
  if (!existsSync(from)) {
    console.error('找不到 ' + from);
    console.error('先在该仓库跑 cargo build --release');
    process.exit(1);
  }
  copyFileSync(from, to);
  console.log('已拷入 ' + to);
}
