#Requires -Version 5.1
# Builds Stellar: runs the tests, builds the release exe and the self-contained
# hardware host, assembles .\dist\Stellar and zips it for a GitHub release.
param([string]$TargetDir = (Join-Path $PSScriptRoot 'target'))
$ErrorActionPreference = 'Stop'
$env:CARGO_TARGET_DIR = $TargetDir
Push-Location $PSScriptRoot
try {
    cargo test --quiet
    if ($LASTEXITCODE) { throw 'Tests failed' }
    cargo build --release
    if ($LASTEXITCODE) { throw 'cargo build failed' }
    $hwOut = Join-Path $TargetDir 'hwhost-publish'
    if (Test-Path $hwOut) { Remove-Item $hwOut -Recurse -Force }
    dotnet publish hwhost/StellarHw.csproj -c Release -o $hwOut --nologo -v quiet
    if ($LASTEXITCODE) { throw 'dotnet publish failed' }

    # Assemble locally, then mirror (deleting trees on network shares is unreliable).
    $stage = Join-Path $TargetDir 'dist-stage\Stellar'
    if (Test-Path (Split-Path $stage)) { Remove-Item (Split-Path $stage) -Recurse -Force }
    New-Item -ItemType Directory (Join-Path $stage 'hw') | Out-Null
    Copy-Item (Join-Path $TargetDir 'release\stellar.exe') $stage
    Copy-Item (Join-Path $hwOut '*') (Join-Path $stage 'hw') -Recurse
    Copy-Item LICENSE, THIRD-PARTY-NOTICES.md $stage

    $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
    $zip = Join-Path $TargetDir "Stellar-$version-win-x64.zip"
    if (Test-Path $zip) { Remove-Item $zip }
    Compress-Archive -Path $stage -DestinationPath $zip

    $dist = Join-Path $PSScriptRoot 'dist'
    robocopy (Split-Path $stage) $dist /MIR /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "Copying to $dist failed (robocopy $LASTEXITCODE)" }
    Copy-Item $zip $dist
    $global:LASTEXITCODE = 0
    Write-Host "Built $dist (and $(Split-Path $zip -Leaf))"
} finally { Pop-Location }
