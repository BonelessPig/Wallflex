<#
.SYNOPSIS
Builds a release of Wallflex and zips it for sharing.

.DESCRIPTION
Runs the checks (fmt, clippy, tests), builds the release exe, and puts
dist\wallflex-<version>.zip together with the exe, the setup guide, the
default settings and the licences. Nothing personal goes in: settings,
Discord tokens and logs live in %APPDATA%\Wallflex, not in the repo.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File tools\package.ps1
#>
param(
    # Skip fmt, clippy and the tests (for a quick rebuild).
    [switch]$SkipChecks
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $root
try {
    function Invoke-Step([string]$Name, [scriptblock]$Step) {
        Write-Host "== $Name"
        & $Step
        if ($LASTEXITCODE -ne 0) { throw "$Name failed ($LASTEXITCODE)" }
    }

    if (-not $SkipChecks) {
        Invoke-Step 'cargo fmt --check' { cargo fmt --check }
        Invoke-Step 'cargo clippy' {
            cargo clippy --all-targets -- -D warnings
        }
        Invoke-Step 'cargo test' { cargo test }
    }
    Invoke-Step 'cargo build --release' { cargo build --release }

    $match = Select-String -Path 'Cargo.toml' -Pattern '^version = "(.+)"' |
        Select-Object -First 1
    if (-not $match) { throw 'no version in Cargo.toml' }
    $version = $match.Matches[0].Groups[1].Value
    $name = "wallflex-$version"
    $stage = Join-Path $root "dist\$name"
    $zip = Join-Path $root "dist\$name.zip"

    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    if (Test-Path $zip) { Remove-Item -Force $zip }
    New-Item -ItemType Directory -Force $stage | Out-Null

    $files = @{
        'target\release\wallflex.exe' = 'wallflex.exe'
        'docs\SETUP.md'               = 'SETUP.md'
        'config.default.toml'         = 'config.example.toml'
        'LICENSE-MIT'                 = 'LICENSE-MIT'
        'LICENSE-APACHE'              = 'LICENSE-APACHE'
    }
    foreach ($source in $files.Keys) {
        Copy-Item $source (Join-Path $stage $files[$source])
    }
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip

    $size = [math]::Round((Get-Item $zip).Length / 1KB)
    Write-Host "== $zip ($size KB)"
}
finally {
    Pop-Location
}
