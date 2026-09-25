@echo off
setlocal EnableExtensions
title EXMACHINA Quickstart

:: Bootstrap shell: the real logic lives in the PowerShell script written by :write_ps1 (PS 5.1 compatible).
:: Usage: quickstart.bat [--port N]   |   --help shows usage (fully local, no network).
:: NOTE: this file is intentionally pure ASCII (no chcp / no CJK here) so cmd's batch
::       parser never hits the codepage 65001 multi-byte line-offset bug.

if /i "%~1"=="--help" goto :help
if /i "%~1"=="-h" goto :help
set "PORT=4173"
if /i "%~1"=="--port" (
  if "%~2"=="" (
    echo [X] --port requires a port number.
    exit /b 1
  )
  set "PORT=%~2"
)

where powershell >nul 2>&1
if errorlevel 1 (
  echo [X] Windows PowerShell not found. It ships with Windows; repair your environment and retry.
  exit /b 1
)

set "PS1=%TEMP%\exm_quickstart.ps1"
call :write_ps1
:: PS 5.1 parses BOM-less files as ANSI: rewrite the generated UTF-8 script with a BOM.
powershell -NoProfile -Command "$c = Get-Content -Raw -Encoding UTF8 '%PS1%'; [System.IO.File]::WriteAllText('%PS1%', $c, (New-Object System.Text.UTF8Encoding $true))"
powershell -NoProfile -ExecutionPolicy Bypass -File "%PS1%" -Port %PORT%
set "RC=%errorlevel%"
del "%PS1%" >nul 2>&1
exit /b %RC%

:write_ps1
>> "%PS1%" echo param([int]$Port = 4173)
>> "%PS1%" echo $ErrorActionPreference = 'Stop'
>> "%PS1%" echo $repo = 'KurohaneKaoruko/ExMachina'
>> "%PS1%" echo $dir = 'exmachina'
>> "%PS1%" echo $tag = $env:EXM_VERSION
>> "%PS1%" echo $t0 = Get-Date
>> "%PS1%" echo function Ok($m) { Write-Host '  [OK] ' -ForegroundColor Green -NoNewline; Write-Host $m }
>> "%PS1%" echo function Info($m) { Write-Host '  ..   ' -ForegroundColor Cyan -NoNewline; Write-Host $m }
>> "%PS1%" echo function Warn($m) { Write-Host '  [!]  ' -ForegroundColor Yellow -NoNewline; Write-Host $m }
>> "%PS1%" echo function Fail($m, $hint) { Write-Host "  [X] $m" -ForegroundColor Red; if ($hint) { Write-Host "      Hint: $hint" -ForegroundColor Yellow }; exit 1 }
>> "%PS1%" echo try {
>> "%PS1%" echo   Write-Host ''
>> "%PS1%" echo   Write-Host '  EX.MACHINA - agent cluster quickstart (prebuilt release)' -ForegroundColor Cyan
>> "%PS1%" echo   Write-Host '  [1/4] Environment preflight...'
>> "%PS1%" echo   $drive = (Get-Location).Path.Substring(0,1)
>> "%PS1%" echo   $freeMB = [math]::Round((Get-PSDrive $drive).Free / 1MB)
>> "%PS1%" echo   if ($freeMB -lt 512) { Fail "not enough free disk space (${freeMB}MB, need 512MB)" 'free up space or switch to another working directory' }
>> "%PS1%" echo   Ok "${freeMB}MB free"
>> "%PS1%" echo   Write-Host '  [2/4] Resolving version and platform...'
>> "%PS1%" echo   $plat = 'windows-x64'
>> "%PS1%" echo   $file = "exmachina-$plat.zip"
>> "%PS1%" echo   if (-not $tag) {
>> "%PS1%" echo     try {
>> "%PS1%" echo       $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest"
>> "%PS1%" echo       $tag = $rel.tag_name
>> "%PS1%" echo     } catch {
>> "%PS1%" echo       Fail 'cannot resolve the latest release (network restricted or rate limited)' 'retry with a pinned version: set EXM_VERSION=v1.2.0 then run quickstart.bat'
>> "%PS1%" echo     }
>> "%PS1%" echo   }
>> "%PS1%" echo   if (-not $tag.StartsWith('v')) { $tag = "v$tag" }
>> "%PS1%" echo   Ok "version $tag on $plat"
>> "%PS1%" echo   Write-Host "  [3/4] Download and verify ($file)..."
>> "%PS1%" echo   $url = "https://github.com/$repo/releases/download/$tag/$file"
>> "%PS1%" echo   $tmp = Join-Path $env:TEMP "exm-quickstart-$PID"
>> "%PS1%" echo   $null = New-Item -ItemType Directory -Path $tmp -Force
>> "%PS1%" echo   Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile "$tmp\$file"
>> "%PS1%" echo   Ok "downloaded in $([math]::Round(((Get-Date) - $t0).TotalSeconds))s"
>> "%PS1%" echo   try {
>> "%PS1%" echo     Invoke-WebRequest -UseBasicParsing -Uri "$url.sha256" -OutFile "$tmp\$file.sha256"
>> "%PS1%" echo     $want = (Get-Content "$tmp\$file.sha256" -TotalCount 1).Split(' ')[0]
>> "%PS1%" echo     $have = (Get-FileHash -Algorithm SHA256 "$tmp\$file").Hash
>> "%PS1%" echo     if ($want -ne $have) { Fail 'checksum mismatch' 'the download may be corrupted or tampered; clear the temp dir and retry' }
>> "%PS1%" echo     Ok 'checksum matches'
>> "%PS1%" echo   } catch {
>> "%PS1%" echo     Warn 'release has no .sha256 sidecar, skipping checksum'
>> "%PS1%" echo   }
>> "%PS1%" echo   Write-Host "  [4/4] Extract and install (dir: $dir)..."
>> "%PS1%" echo   if (Test-Path $dir) { Info 'existing install detected -> upgrading (program files replaced, .exmachina data preserved)' }
>> "%PS1%" echo   $null = New-Item -ItemType Directory -Path $dir -Force
>> "%PS1%" echo   $stage = Join-Path $dir ".stage-$PID"
>> "%PS1%" echo   Expand-Archive -Path "$tmp\$file" -DestinationPath $stage -Force
>> "%PS1%" echo   Copy-Item -Path "$stage\*" -Destination $dir -Recurse -Force
>> "%PS1%" echo   Remove-Item $stage -Recurse -Force
>> "%PS1%" echo   Remove-Item $tmp -Recurse -Force
>> "%PS1%" echo   $busy = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
>> "%PS1%" echo   if ($busy) { Fail "port $Port is already in use" 'pick another port: quickstart.bat --port 8173, or stop the process holding it' }
>> "%PS1%" echo   $elapsed = [math]::Round(((Get-Date) - $t0).TotalSeconds)
>> "%PS1%" echo   Write-Host ''
>> "%PS1%" echo   Write-Host '  ==============================================' -ForegroundColor Green
>> "%PS1%" echo   Write-Host '  [OK] Ready!' -ForegroundColor Green
>> "%PS1%" echo   Write-Host "     version   $tag ($plat)"
>> "%PS1%" echo   Write-Host "     dir       $((Get-Location).Path)\$dir"
>> "%PS1%" echo   Write-Host "     port      $Port    elapsed   ${elapsed}s"
>> "%PS1%" echo   Write-Host '  ==============================================' -ForegroundColor Green
>> "%PS1%" echo   Write-Host ''
>> "%PS1%" echo   Write-Host "  Open  http://127.0.0.1:$Port  in your browser."
>> "%PS1%" echo   Write-Host '  First run: open the Models page, add a profile, pick a vendor, paste your API key, set it as default.'
>> "%PS1%" echo   Write-Host '  Stop: Ctrl+C'
>> "%PS1%" echo   $env:EXM_PORT = "$Port"
>> "%PS1%" echo   Set-Location $dir
>> "%PS1%" echo   .\exm-gateway.exe
>> "%PS1%" echo } catch {
>> "%PS1%" echo   Write-Host "  [X] quickstart failed: $_" -ForegroundColor Red
>> "%PS1%" echo   Write-Host '      Fallback: run install.bat to build from source (works on any platform).' -ForegroundColor Yellow
>> "%PS1%" echo   exit 1
>> "%PS1%" echo }
goto :eof

:help
echo EXMACHINA quickstart (Windows: cmd bootstrap + PowerShell runner, no Rust/Node needed)
echo.
echo Usage: quickstart.bat [--port N]
echo.
echo Options:
echo   --port N    gateway listen port (default 4173)
echo   --help      show this help (fully local, no network)
echo.
echo Environment variables (read by the PowerShell section):
echo   EXM_VERSION   pin a release version (e.g. v1.2.0); default is GitHub Releases latest
echo   EXM_PORT      gateway listen port (--port overrides)
echo.
echo Notes: re-running inside an existing install dir upgrades in place (program files
echo        replaced, the .exmachina data directory is preserved); when done, open
echo        http://127.0.0.1:4173 and configure your API key on the Models page.
exit /b 0
