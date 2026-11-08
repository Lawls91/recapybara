<#
release.ps1 - cut a release: bump the app version everywhere it is kept, commit, and tag vX.Y.Z.
Pushing the tag runs .github/workflows/release.yml, which builds, signs and publishes the MSI and latest.json.
Usage: pwsh -File scripts\release.ps1 0.2.0   (then: git push origin main v0.2.0)
#>
param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (git -C $repo status --porcelain) { throw 'The working tree has changes; commit or set them aside first.' }

node (Join-Path $repo 'scripts\release.mjs') bump $Version
if ($LASTEXITCODE -ne 0) { throw 'bump failed' }
node (Join-Path $repo 'scripts\release.mjs') check "v$Version" | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'the bumped versions disagree' }

git -C $repo commit -am "Release v$Version"
if ($LASTEXITCODE -ne 0) { throw 'commit failed' }
git -C $repo tag -a "v$Version" -m "Release v$Version"
if ($LASTEXITCODE -ne 0) { throw 'tag failed' }
Write-Host "Tagged v$Version. Publish it with: git push origin main v$Version"
