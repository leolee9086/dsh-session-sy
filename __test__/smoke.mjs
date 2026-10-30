// 冒烟：确认仓库里随包发布的二进制能用（不依赖 ../rebec/target）。
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { join } from 'node:path';

const here = join(import.meta.dirname, '..');
const require = createRequire(import.meta.url);
const rebec = require(join(here, 'addon', 'rebec.node'));

console.log('addon 版本  =', rebec.version());
const tree = JSON.parse(rebec.parseMarkdownToJson('# hi\n', 'default'));
console.log('addon 解析  =', tree.Type);

const out = execFileSync(join(here, 'bin', 'rebec.exe'), ['parse', '# hi', '--config', 'default'], { encoding: 'utf8' });
console.log('bin 解析    =', JSON.parse(out).Type);
