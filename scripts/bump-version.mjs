// 发版版本号自动同步（AGENTS.md「版本号约定」）。
//   node scripts/bump-version.mjs        → 当前版本 patch +1（如 0.0.2 → 0.0.3）
//   node scripts/bump-version.mjs 0.1.0  → 显式设为指定版本
// 三处必须保持一致：package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml。
// 用正则原位替换，保留文件原有格式；Cargo.lock 由 cargo build 自行跟进。
import { readFileSync, writeFileSync } from 'node:fs';

const FILES = [
  { path: 'package.json', re: /("version"\s*:\s*")([^"]+)(")/ },
  { path: 'src-tauri/tauri.conf.json', re: /("version"\s*:\s*")([^"]+)(")/ },
  // Cargo.toml 里依赖项也有 version 字段，[package] 的永远在第一处
  { path: 'src-tauri/Cargo.toml', re: /(^version\s*=\s*")([^"]+)(")/m },
];

const SEMVER = /^\d+\.\d+\.\d+$/;

const pkg = JSON.parse(readFileSync('package.json', 'utf8'));
const current = pkg.version;
if (!SEMVER.test(current)) {
  console.error(`package.json 版本号无法解析：${current}`);
  process.exit(1);
}

let next = process.argv[2];
if (next) {
  if (!SEMVER.test(next)) {
    console.error(`显式版本号必须是 x.y.z 形式：${next}`);
    process.exit(1);
  }
} else {
  const [major, minor, patch] = current.split('.').map(Number);
  next = `${major}.${minor}.${patch + 1}`;
}

if (next === current) {
  console.log(`版本号未变化：${current}`);
  process.exit(0);
}

for (const { path, re } of FILES) {
  const text = readFileSync(path, 'utf8');
  const m = re.exec(text);
  if (!m) {
    console.error(`${path} 里找不到版本号字段`);
    process.exit(1);
  }
  writeFileSync(path, text.slice(0, m.index) + m[1] + next + m[3] + text.slice(m.index + m[0].length));
  console.log(`${path}: ${m[2]} → ${next}`);
}
