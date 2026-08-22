// 把 release 裸 exe 复制成带版本号的文件名（如 wardex_0.0.7.exe），
// 版本号读 package.json（与 NSIS 安装器 WarDex_<版本>_x64-setup.exe 同源，
// bump-version.mjs 已保证三处版本一致）。
// 用法：node scripts/version-exe.mjs
// 退出码：0 成功；1 找不到源二进制或版本号非法。
import { readFileSync, copyFileSync, existsSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');

const pkg = JSON.parse(readFileSync(resolve(repoRoot, 'package.json'), 'utf8'));
const version = pkg.version;
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`[version-exe] package.json 版本号无法解析：${version}`);
  process.exit(1);
}

const exe = process.platform === 'win32' ? 'wardex-tauri.exe' : 'wardex-tauri';
const outName = process.platform === 'win32' ? `wardex_${version}.exe` : `wardex_${version}`;
const src = resolve(repoRoot, 'src-tauri', 'target', 'release', exe);
const dest = resolve(repoRoot, 'src-tauri', 'target', 'release', outName);

if (!existsSync(src)) {
  console.error(`[version-exe] 找不到 release 二进制：${src}`);
  process.exit(1);
}
copyFileSync(src, dest);
console.log(`[version-exe] ${src} → ${dest}`);
