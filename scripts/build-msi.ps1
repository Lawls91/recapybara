<#
build-msi.ps1 - build the release app and its per-user MSI: out\<productName>-<version>.msi.
  1. npx tauri build --no-bundle (release, custom-protocol: the frontend is embedded in the exe)
  2. wix build installer\wix\Package.wxs (WiX 5.0.2 dotnet global tool, installed from NuGet if missing)
productName and version come from src-tauri\tauri.conf.json, so a rename touches only productName.
No custom actions, no signing, no downloads from GitHub. Usage: pwsh -File scripts\build-msi.ps1
#>
$ErrorActionPreference = 'Stop'
# No workload is used; stop the SDK starting a workload-set install (an elevation prompt) on restore.
$env:MSBuildEnableWorkloadResolver        = 'false'
$env:DOTNET_SKIP_WORKLOAD_INTEGRITY_CHECK = '1'
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }

$repo = Split-Path -Parent $PSScriptRoot
$conf = Get-Content (Join-Path $repo 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
$name, $version = $conf.productName, $conf.version
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "tauri.conf.json version '$version' is not MAJOR.MINOR.PATCH" }

$wix = Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe'
$wixVersion = '5.0.2'
# Add nuget.org and skip unreachable configured feeds (a private feed off-VPN must not fail the install).
$nuget = '--add-source', 'https://api.nuget.org/v3/index.json', '--ignore-failed-sources'
if (-not (Test-Path $wix)) {
    dotnet tool install --global wix --version $wixVersion @nuget
} elseif (-not ((& $wix --version) -like "$wixVersion*")) {
    dotnet tool update --global wix --version $wixVersion @nuget
}
if ($LASTEXITCODE -ne 0) { throw "could not install the WiX $wixVersion dotnet tool" }

$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
Push-Location $repo
try {
    # An incremental build can keep stale embedded frontend assets; rebuild the app crate itself.
    cargo clean -p meeting-capture --release
    Write-Host "==> tauri build --no-bundle ($name $version)"
    npx tauri build --no-bundle
    if ($LASTEXITCODE -ne 0) { throw 'tauri build failed' }
} finally { Pop-Location }

$exe = Join-Path $target 'release\meeting-capture.exe'
if (-not (Test-Path $exe)) { throw "app exe not produced: $exe" }
$payload = Join-Path $repo 'out\payload'
Remove-Item $payload -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $payload | Out-Null
Copy-Item $exe $payload

$msi = Join-Path $repo "out\$name-$version.msi"
Write-Host "==> wix build -> $msi"
& $wix build (Join-Path $repo 'installer\wix\Package.wxs') `
    -d "ProductName=$name" -d "Version=$version" -d "PayloadDir=$payload" `
    -d "IconFile=$(Join-Path $repo 'src-tauri\icons\icon.ico')" `
    -o $msi
if ($LASTEXITCODE -ne 0) { throw 'wix build failed' }
'{0:N1} MB  {1}' -f ((Get-Item $msi).Length / 1MB), $msi | Write-Host
