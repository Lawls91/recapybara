<#
run-dev.ps1: launches a dev instance of Meeting capture beside the installed copy (docs/development.md, "Dev instances").

  pwsh -File scripts\run-dev.ps1 [-StateDir <dir>] [-Build] [-Claude <exe>] [-Scenario <file>]

All its state lives under -StateDir (default %LOCALAPPDATA%\MeetingCapture-dev\default). It runs a copy of the
debug exe from <StateDir>\bin, so the build output stays unlocked, and prints
{pid, stateDir} as JSON. Re-running it replaces that dir's previous instance, stopped by its recorded PID and only
when that PID still runs the copy: never by name, so the installed copy and other dirs' instances are left alone.
-Claude and -Scenario point it at the fake claude (MEETING_CAPTURE_CLAUDE, FAKE_CLAUDE_SCENARIO); -Claude also sets
MEETING_CAPTURE_HOME to <StateDir>\home unless it is set, so a fake run stays out of your ~/.claude.
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
        # stdout carries only the JSON line, so everything else goes to stderr.
        npx tauri build --debug --no-bundle 2>&1 | ForEach-Object { [Console]::Error.WriteLine($_) }
        if ($LASTEXITCODE) { throw "build failed (exit $LASTEXITCODE)" }
    } finally { Pop-Location }
}

# Which build runs, so a stale exe in another target dir is seen before a test of new code is read wrongly.
[Console]::Error.WriteLine("run-dev: running $built (built $((Get-Item -LiteralPath $built).LastWriteTime.ToString('yyyy-MM-dd HH:mm')))")
$exe = Join-Path $StateDir 'bin\meeting-capture.exe'
$pidFile = Join-Path $StateDir 'dev.pid'
$oldId = 0
# A missing or corrupt dev.pid means no previous instance.
if ((Test-Path -LiteralPath $pidFile) -and [int]::TryParse("$(Get-Content -LiteralPath $pidFile -Raw)".Trim(), [ref]$oldId)) {
    $old = Get-Process -Id $oldId -ErrorAction SilentlyContinue
    if ($old -and $old.Path -and ([IO.Path]::GetFullPath($old.Path) -ieq $exe)) {
        [Console]::Error.WriteLine("run-dev: stopping this dir's previous instance (pid $($old.Id))")
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
if ($Claude) {
    $env:MEETING_CAPTURE_CLAUDE = [IO.Path]::GetFullPath($Claude)
    # A fake claude writes its project folder under the home's .claude: keep that in the state dir too.
    if (-not $env:MEETING_CAPTURE_HOME) { $env:MEETING_CAPTURE_HOME = Join-Path $StateDir 'home' }
}
if ($Scenario) { $env:FAKE_CLAUDE_SCENARIO = [IO.Path]::GetFullPath($Scenario) }
# The debug exe is a console app: its console is hidden. No redirects: they make the app inherit this script's
# handles, so a caller reading our output would wait for the app to exit.
$p = Start-Process -FilePath $exe -WindowStyle Hidden -PassThru
Set-Content -LiteralPath $pidFile -Value $p.Id
[pscustomobject]@{ pid = $p.Id; stateDir = $StateDir } | ConvertTo-Json -Compress
