@echo off
rem WarDex 开发版启动（Vite 热更新 + Tauri debug，数据目录 WarDex-tauri-dev）
cd /d "%~dp0"
npm run tauri dev
if errorlevel 1 pause
