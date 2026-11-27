#Requires -Version 5.1
# Builds Stellar (release) and its hardware host into .\dist, then runs the tests.
param([string]$TargetDir = (Join-Path $PSScriptRoot 'target'))
$ErrorActionPreference = 'Stop'
$env:CARGO_TARGET_DIR = $TargetDir
Push-Location $PSScriptRoot
try {
    cargo test --quiet
    if ($LASTEXITCODE) { throw 'Tests failed' }
    cargo build --release
    if ($LASTEXITCODE) { throw 'cargo build failed' }
    dotnet publish hwhost/StellarHw.csproj -c Release -o (Join-Path $TargetDir 'hwhost-publish') --nologo -v quiet
    if ($LASTEXITCODE) { throw 'dotnet publish failed' }
    # Assemble locally, then mirror (deleting trees on network shares is unreliable).
    $stage = Join-Path $TargetDir 'dist-stage'
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    New-Item -ItemType Directory (Join-Path $stage 'hw') | Out-Null
    Copy-Item (Join-Path $TargetDir 'release\stellar.exe') $stage
    Copy-Item (Join-Path $TargetDir 'hwhost-publish\*') (Join-Path $stage 'hw') -Recurse
    $dist = Join-Path $PSScriptRoot 'dist'
    robocopy $stage $dist /MIR /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "Copying to $dist failed (robocopy $LASTEXITCODE)" }
    $global:LASTEXITCODE = 0
    Write-Host "Built $dist"
} finally { Pop-Location }
