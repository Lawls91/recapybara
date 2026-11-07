<#
run-dev.ps1: launches a dev instance of Meeting capture beside the installed copy (README, "Dev instances").

  pwsh -File scripts\run-dev.ps1 [-StateDir <dir>] [-Build] [-Claude <exe>] [-Scenario <file>]

All its state lives under -StateDir (default %LOCALAPPDATA%\MeetingCapture-dev\default). It runs a copy of the
debug exe from <StateDir>\bin, so the build output stays unlocked, logs to <StateDir>\dev.log, and prints
{pid, stateDir} as JSON. Re-running it replaces that dir's previous instance, stopped by its recorded PID and only
when that PID still runs the copy: never by name, so the installed copy and other dirs' instances are left alone.
-Claude and -Scenario point it at the fake claude (MEETING_CAPTURE_CLAUDE, FAKE_CLAUDE_SCENARIO).
#>
[CmdletBinding()]
param(
    [string]$StateDir = (Join-Path $env:LOCALAPPDATA 'MeetingCapture-dev\default'),
    [switch]$Build,
    [string]$Claude,
    [string]$Scenario
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$StateDir = [IO.Path]::GetFullPath($StateDir)

$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
$built = Join-Path $target 'debug\meeting-capture.exe'
if ($Build -or -not (Test-Path -LiteralPath $built)) {
    $env:PATH = "$HOME\.cargo\bin;$env:PATH"
    Push-Location $repo
    try {
        npx tauri build --debug --no-bundle | Out-Host
        if ($LASTEXITCODE) { throw "build failed (exit $LASTEXITCODE)" }
    } finally { Pop-Location }
}

$exe = Join-Path $StateDir 'bin\meeting-capture.exe'
$pidFile = Join-Path $StateDir 'dev.pid'
if (Test-Path -LiteralPath $pidFile) {
    $old = Get-Process -Id ([int](Get-Content -LiteralPath $pidFile -Raw)) -ErrorAction SilentlyContinue
    if ($old -and $old.Path -and ([IO.Path]::GetFullPath($old.Path) -ieq $exe)) {
        Write-Host "run-dev: stopping this dir's previous instance (pid $($old.Id))"
        Stop-Process -Id $old.Id -Force
        $old.WaitForExit(10000) | Out-Null
    }
}

New-Item -ItemType Directory -Force (Split-Path $exe) | Out-Null
# The old image's lock can outlive its process by a moment.
foreach ($i in 1..20) {
    try { Copy-Item -LiteralPath $built -Destination $exe -Force; break }
    catch { if ($i -eq 20) { throw }; Start-Sleep -Milliseconds 250 }
}

$env:MEETING_CAPTURE_DEV = $StateDir
if ($Claude) { $env:MEETING_CAPTURE_CLAUDE = [IO.Path]::GetFullPath($Claude) }
if ($Scenario) { $env:FAKE_CLAUDE_SCENARIO = [IO.Path]::GetFullPath($Scenario) }
# The debug exe is a console app: its console is hidden and its output goes to dev.log.
$log = Join-Path $StateDir 'dev.log'
$p = Start-Process -FilePath $exe -WindowStyle Hidden -PassThru -RedirectStandardOutput $log -RedirectStandardError "$log.err"
Set-Content -LiteralPath $pidFile -Value $p.Id
[pscustomobject]@{ pid = $p.Id; stateDir = $StateDir } | ConvertTo-Json -Compress
