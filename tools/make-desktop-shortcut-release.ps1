$exe = 'C:\workspace\Wardex-rust\src-tauri\target\release\wardex-tauri.exe'
$lnk = [Environment]::GetFolderPath('Desktop') + '\WarDex.lnk'
$s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
$s.TargetPath = $exe
$s.WorkingDirectory = Split-Path $exe
$s.Save()
Write-Output "shortcut created: $lnk -> $exe"
