@echo off
rem WarDex 正式版打包（release exe + NSIS 安装器，默认内置 pi）
rem 每次打包先把三处版本号 patch +1（scripts/bump-version.mjs）；
rem 显式指定版本：node scripts/bump-version.mjs 0.1.0 后再跑本脚本。
rem 默认走 build:with-pi，且 WARDEX_PI_REBUILD=1 全量重编 pi（重建依赖子包
rem + coding-agent + bun 编译自包含二进制 + 资产），保证把最新 pi 打进安装包。
rem 不想重编 pi（用现有产物增量打包）：set WARDEX_PI_REBUILD=0。
rem 不带 pi：set WARDEX_BUNDLE_PI=0。
rem 产物：
rem   src-tauri\target\release\wardex_<版本>.exe   （带版本号的裸 exe，version-exe.mjs 生成）
rem   src-tauri\target\release\wardex-tauri.exe   （cargo 原始二进制，保留）
rem   src-tauri\target\release\bundle\nsis\        （WarDex_<版本>_x64-setup.exe 安装器）
cd /d "%~dp0"
node scripts/bump-version.mjs || exit /b 1
set WARDEX_PI_REBUILD=1
npm run build:with-pi || exit /b 1
node scripts/version-exe.mjs || exit /b 1
pause
