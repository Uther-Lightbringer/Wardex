@echo off
rem WarDex portable build (dist-portable\ + WarDex-win64-<version>.zip, pi bundled).
rem Prerequisite: src-tauri\target\release\wardex-tauri.exe already built
rem (via build-release.bat, npm run tauri build, or cargo build --release).
rem If pi dist (pi-runtime\packages\coding-agent\dist\pi.exe) is missing,
rem this script runs scripts\bundle-pi.mjs to build it.
rem Force full pi rebuild: set WARDEX_PI_REBUILD=1 before running.
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

if not exist "%RELEASE_EXE%" (
  echo [build-portable] ERROR: %RELEASE_EXE% not found. Run build-release.bat ^(or npm run tauri build^) first.
  exit /b 1
)

if "%WARDEX_PI_REBUILD%"=="1" (
  echo [build-portable] WARDEX_PI_REBUILD=1, rebuilding pi from source...
  node scripts\bundle-pi.mjs || exit /b 1
) else if not exist "%PI_DIST%\pi.exe" (
  echo [build-portable] pi dist not found, running bundle-pi.mjs...
  node scripts\bundle-pi.mjs || exit /b 1
)

rem --- Assemble dist-portable (overwrite exe; mirror resources\pi to drop stale files)
if not exist "%OUT%" mkdir "%OUT%"
copy /y "%RELEASE_EXE%" "%OUT%\wardex.exe" >nul || exit /b 1
copy /y "background.example.json" "%OUT%\" >nul

echo [build-portable] syncing pi to %OUT%\resources\pi ...
robocopy "%PI_DIST%" "%OUT%\resources\pi\packages\coding-agent\dist" /MIR /NFL /NDL /NJH /NJS /NP
if errorlevel 8 exit /b 1

echo [build-portable] syncing pi-extensions ...
robocopy "pi-extensions" "%OUT%\resources\pi-extensions" /E /NFL /NDL /NJH /NJS /NP
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
