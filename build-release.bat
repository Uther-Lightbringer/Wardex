@echo off
rem WarDex 正式版打包（release exe + NSIS 安装器）
rem 产物：
rem   src-tauri\target\release\wardex.exe
rem   src-tauri\target\release\bundle\nsis\
cd /d "%~dp0"
npm run tauri build
pause
