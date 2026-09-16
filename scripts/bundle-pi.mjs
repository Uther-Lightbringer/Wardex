#!/usr/bin/env node
// bundle-pi.mjs
//
// 把 workspace 的 pi 源码编译成一个自包含的「pi 二进制 + 资产目录」，供
// Wardex 打包时通过 tauri bundle.resources 打进安装包。
//
// 方案 II（免 Node）：
//   - bun build --compile 把 pi-coding-agent 连同全部依赖 + 内置 Node 运行时
//     编译成单个可执行文件（pi.exe），用户端不再需要安装 Node。
//   - copy-binary-assets 把 theme/ assets/ export-html/ docs/ examples/ wasm
//     等静态资产拷到 dist/，二进制运行时要按自身相对路径读取它们。
//
// 输出布局（与 chat/pi.rs locate_plugin_dir 的预期一致）：
//   pi-runtime/packages/coding-agent/dist/<pi.exe|pi> + 资产
//
// 用法：
//   node scripts/bundle-pi.mjs            # 用默认 workspace 同级 pi 目录
//   node scripts/bundle-pi.mjs <piDir>    # 指定 pi 源码目录
//   WARDEX_BUNDLE_PI=0 node scripts/bundle-pi.mjs   # 跳过（不带 pi）
//   WARDEX_PI_REBUILD=1 node scripts/bundle-pi.mjs  # 全量重编 pi（离线 build:offline）
//
// 退出码：0 成功；非 0 失败（由 tauri beforeBuildCommand 捕获）。

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, copyFileSync, statSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');

// --- 全量重编开关：WARDEX_PI_REBUILD=1 时走 pi 根目录的 build:offline（离线
// 重建依赖子包 tui/telemetry/ai/agent/protocol/client + coding-agent，不联网
// fetch models.dev，再 bun 编译自包含二进制 + 拷贝资产），否则按需增量编译。--
const fullRebuild = process.env.WARDEX_PI_REBUILD === '1';

// --- 可开关：WARDEX_BUNDLE_PI=0 表示本次打包不带 pi -----------------------
if (process.env.WARDEX_BUNDLE_PI === '0') {
  console.log('[bundle-pi] WARDEX_BUNDLE_PI=0，跳过 pi 打包（安装包不含 pi）。');
  process.exit(0);
}

// --- 定位 pi 源码目录 -----------------------------------------------------
const argvPi = process.argv[2];
const candidates = [
  argvPi && resolve(argvPi),
  process.env.WARDEX_PI_DIR,
  join(repoRoot, '..', 'pi'), // workspace 同级（C:\workspace\pi）
];
const piRoot = candidates.find((c) => c && existsSync(join(c, 'packages/coding-agent')));
if (!piRoot) {
  console.error(
    `[bundle-pi] 找不到 pi 源码（需含 packages/coding-agent）。` +
      `已尝试：${candidates.filter(Boolean).join(', ')}。\n` +
      `可用参数指定：node scripts/bundle-pi.mjs <piDir>` +
      `，或设 WARDEX_BUNDLE_PI=0 跳过。`
  );
  process.exit(2);
}
console.log(`[bundle-pi] 使用 pi 源码目录：${piRoot}`);

const agentPkg = join(piRoot, 'packages', 'coding-agent');
const srcDist = join(agentPkg, 'dist');
const outRoot = join(repoRoot, 'pi-runtime');
const outAgent = join(outRoot, 'packages', 'coding-agent');
const outDist = join(outAgent, 'dist');
const exe = process.platform === 'win32' ? 'pi.exe' : 'pi';
const srcExe = join(srcDist, exe);

// --- 增量模式下校验五个子包 dist（全量重编模式由 build:binary 重建） -----
if (!fullRebuild) {
  const neededDeps = ['ai', 'tui', 'agent', 'protocol', 'client'];
  const missingDeps = neededDeps.filter((d) => !existsSync(join(piRoot, 'packages', d, 'dist')));
  if (missingDeps.length > 0) {
    console.error(
      `[bundle-pi] pi 依赖子包未构建：${missingDeps.join(', ')}。` +
        `请先在 pi 仓库构建：npm install && npm run build（或 build:binary），` +
        `或设 WARDEX_PI_REBUILD=1 全量重编。`
    );
    process.exit(3);
  }
}

function run(cmd, args, opts) {
  // Windows 上 npm/bun/robocopy 多为 .cmd/.exe 包装，走 shell 才能被找到。
  const r = spawnSync(cmd, args, { stdio: 'inherit', shell: process.platform === 'win32', ...opts });
  if (r.status !== 0) {
    console.error(`[bundle-pi] 命令失败 (${r.status})：${cmd} ${args.join(' ')}`);
    process.exit(4);
  }
}

// 判断是否需要重编：coding-agent/src 下任一文件比现有二进制新。
function needsRebuild(exePath) {
  if (!existsSync(exePath)) return true;
  const binTime = statSync(exePath).mtimeMs;
  const srcDir = join(agentPkg, 'src');
  if (!existsSync(srcDir)) return false;
  let newest = 0;
  const walk = (dir) => {
    for (const f of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, f.name);
      if (f.isDirectory()) walk(full);
      else if (f.isFile() && statSync(full).mtimeMs > newest) newest = statSync(full).mtimeMs;
    }
  };
  walk(srcDir);
  return newest > binTime;
}

// --- 1) 编译二进制 + 拷贝资产 ----------------------------------------------
if (fullRebuild) {
  // 全量重编（离线）：走 pi 根目录 build:offline —— 用仓库内已固化的模型数据
  // 重建全部依赖子包 + coding-agent（ai 子包用 build:offline，不联网 fetch
  // models.dev；联网超时会把 build:binary 卡死在 generate-models），再 bun
  // 编译自包含二进制 + 拷贝二进制资产。build:offline 比 build:binary 多构建
  // server / session-backends/sqlite-node 两个离线子包，代价可忽略。
  console.log('[bundle-pi] 全量重编 pi（离线：npm run build:offline + bun 编译 + copy-binary-assets）…');
  run('npm', ['run', 'build:offline'], { cwd: piRoot });
  run(
    'bun',
    [
      'build', '--compile',
      './dist/bun/cli.js',
      './src/utils/image-resize-worker.ts',
      '--outfile', `dist/${exe}`,
    ],
    { cwd: agentPkg }
  );
  run('npm', ['run', 'copy-binary-assets'], { cwd: agentPkg });
} else {
  // 增量：只编译、不重建五个子包（它们的 dist 已存在）；跳过 build:binary 里的
  // 逐个 rebuild，加快打包。产物落在 srcDist/<exe>。当 coding-agent/src 比
  // 现有二进制新时自动重编（改过 pi 代码就带上新版）。
  if (needsRebuild(srcExe)) {
    console.log('[bundle-pi] 检测到源码更新，编译 pi 二进制（bun build --compile）…');
    run(
      'bun',
      [
        'build', '--compile',
        './dist/bun/cli.js',
        './src/utils/image-resize-worker.ts',
        '--outfile', `dist/${exe}`,
      ],
      { cwd: agentPkg }
    );
  } else {
    console.log(`[bundle-pi] 二进制已是最新（${srcExe}），跳过编译。`);
  }

  // --- 2) copy-binary-assets ---
  console.log('[bundle-pi] 拷贝静态资产（copy-binary-assets）…');
  run('npm', ['run', 'copy-binary-assets'], { cwd: agentPkg });
}

// --- 3) 收敛到 pi-runtime ---------------------------------------------------
console.log('[bundle-pi] 收敛产物到 pi-runtime/ …');
mkdirSync(outDist, { recursive: true });

// 资产目录（二进制按相对自身路径读取）
for (const sub of ['theme', 'assets', 'export-html', 'docs', 'examples']) {
  const s = join(srcDist, sub);
  if (!existsSync(s)) continue;
  if (process.platform === 'win32') {
    // robocopy 退出码 0-7 表示成功（>0 是有文件被复制），≥8 才是失败
    const r = spawnSync('robocopy', [s, join(outDist, sub), '/E'], { stdio: 'inherit' });
    if (r.status !== undefined && r.status >= 8) {
      console.error(`[bundle-pi] robocopy 失败 (${r.status})：${sub}`);
      process.exit(4);
    }
  } else {
    run('cp', ['-r', s, join(outDist, sub)]);
  }
}
// 二进制 + wasm + 文档
for (const f of [exe, 'photon_rs_bg.wasm', 'package.json', 'README.md', 'CHANGELOG.md']) {
  const s = join(srcDist, f);
  if (existsSync(s)) copyFileSync(s, join(outDist, f));
}

console.log(`[bundle-pi] 完成。产物：${outRoot}`);
// Explicit 0: on Windows the last robocopy (exit 1 = files copied) can
// otherwise leak into this process's exit code and abort the caller.
process.exit(0);
