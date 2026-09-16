@echo off
rem WarDex portable build (dist-portable\ + WarDex-win64-<version>.zip, pi bundled).
rem Each run bumps patch version (package.json / tauri.conf.json / Cargo.toml)
rem via scripts\bump-version.mjs, then builds CURRENT source with
rem `npx tauri build --no-bundle` (production frontend embedded; cargo
rem build --release alone still uses devUrl localhost:1420 = black screen)
rem and packs. Does NOT build the NSIS installer (use build-release.bat).
rem Explicit version: node scripts\bump-version.mjs 0.1.0 then run this script
rem with WARDEX_PORTABLE_SKIP_BUMP=1.
rem Pi: always runs scripts\bundle-pi.mjs (incremental; recompiles pi.exe if
rem coding-agent src is newer). Full pi rebuild: set WARDEX_PI_REBUILD=1.
rem Skip pi: set WARDEX_BUNDLE_PI=0.
rem Skip the app rebuild (use existing release exe): set WARDEX_PORTABLE_SKIP_BUILD=1.
rem Outputs:
rem   dist-portable\wardex.exe + resources\pi\...   (portable dir, pi ready to use)
rem   WarDex-win64-<version>.zip                    (version from package.json)
rem Note: dist-portable root is NOT cleaned, so make-desktop-shortcut.bat survives.
rem Keep this file pure ASCII + CRLF: cmd.exe parses .bat in the OEM codepage
rem (GBK on zh-CN systems) and UTF-8 Chinese comments corrupt parsing.
setlocal
cd /d "%~dp0"

set "RELEASE_EXE=src-tauri\target\release\wardex-tauri.exe"
set "PI_DIST=pi-runtime\packages\coding-agent\dist"
set "OUT=dist-portable"

if "%WARDEX_PORTABLE_SKIP_BUMP%"=="1" (
  echo [build-portable] WARDEX_PORTABLE_SKIP_BUMP=1, keeping current version.
) else (
  echo [build-portable] bumping patch version...
  node scripts\bump-version.mjs || exit /b 1
)

rem --- Latest pi into pi-runtime (incremental unless WARDEX_PI_REBUILD=1)
if not "%WARDEX_BUNDLE_PI%"=="0" (
  echo [build-portable] bundling pi...
  call node scripts\bundle-pi.mjs
  if errorlevel 1 exit /b 1
)

rem --- Latest app binary from current source
if "%WARDEX_PORTABLE_SKIP_BUILD%"=="1" (
  echo [build-portable] WARDEX_PORTABLE_SKIP_BUILD=1, using existing release exe.
) else (
  echo [build-portable] building app ^(tauri build --no-bundle^)...
  call npx tauri build --no-bundle || exit /b 1
)

if not exist "%RELEASE_EXE%" (
  echo [build-portable] ERROR: %RELEASE_EXE% not found.
  exit /b 1
)

if not "%WARDEX_BUNDLE_PI%"=="0" if not exist "%PI_DIST%\pi.exe" (
  echo [build-portable] ERROR: %PI_DIST%\pi.exe not found after bundle-pi.
  exit /b 1
)

rem --- Assemble dist-portable (overwrite exe; mirror resources\pi to drop stale files)
if not exist "%OUT%" mkdir "%OUT%"
copy /y "%RELEASE_EXE%" "%OUT%\wardex.exe" >nul || exit /b 1
copy /y "background.example.json" "%OUT%\" >nul

if not "%WARDEX_BUNDLE_PI%"=="0" (
  echo [build-portable] syncing pi to %OUT%\resources\pi ...
  robocopy "%PI_DIST%" "%OUT%\resources\pi\packages\coding-agent\dist" /MIR /NFL /NDL /NJH /NJS /NP
  if errorlevel 8 exit /b 1
)

echo [build-portable] syncing pi-extensions ...
robocopy "pi-extensions" "%OUT%\resources\pi-extensions" /E /NFL /NDL /NJH /NJS /NP
if errorlevel 8 exit /b 1

echo [build-portable] syncing pi-packages (pi-multiagent) ...
robocopy "pi-packages" "%OUT%\resources\pi-packages" /E /NFL /NDL /NJH /NJS /NP
if errorlevel 8 exit /b 1

rem --- Version from package.json, then zip
for /f "delims=" %%v in ('node -p "require('./package.json').version"') do set "VER=%%v"
set "ZIP=WarDex-win64-%VER%.zip"
if exist "%ZIP%" del /f "%ZIP%"

echo [build-portable] creating %ZIP% ...
powershell -NoProfile -Command "Compress-Archive -Path '%OUT%\*' -DestinationPath '%ZIP%' -CompressionLevel Optimal" || exit /b 1

echo [build-portable] done:
echo   %OUT%\           portable dir (resources\pi sits next to wardex.exe, pi works out of the box)
echo   %ZIP%            portable zip
endlocal
