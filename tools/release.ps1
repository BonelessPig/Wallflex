<#
.SYNOPSIS
Publishes a GitHub release of the version in Cargo.toml.

.DESCRIPTION
Versions follow Semantic Versioning, tagged vX.Y.Z (see CHANGELOG.md).
To release:

  1. Set the new version in Cargo.toml and run `cargo build` (updates
     Cargo.lock).
  2. In CHANGELOG.md, move the Unreleased notes under a new
     "## [X.Y.Z] - YYYY-MM-DD" heading and add its link at the bottom.
  3. Commit and push.
  4. Run this script.

It checks the tree is clean and pushed, that the tag doesn't exist yet
and that CHANGELOG.md has notes for the version; then runs
tools\package.ps1 (checks, release build, zip), tags vX.Y.Z, pushes the
tag and creates the GitHub release with the zip and those notes.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File tools\release.ps1
#>
param(
    # Show what would happen without tagging or publishing.
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $root
try {
    function Fail([string]$Message) { throw "release: $Message" }

    $gh = (Get-Command gh -ErrorAction SilentlyContinue).Source
    if (-not $gh) {
        $gh = Join-Path $env:ProgramFiles 'GitHub CLI\gh.exe'
        if (-not (Test-Path $gh)) { Fail 'the GitHub CLI (gh) is needed' }
    }

    $match = Select-String -Path 'Cargo.toml' -Pattern '^version = "(.+)"' |
        Select-Object -First 1
    if (-not $match) { Fail 'no version in Cargo.toml' }
    $version = $match.Matches[0].Groups[1].Value
    if ($version -notmatch '^\d+\.\d+\.\d+$') {
        Fail "version '$version' isn't X.Y.Z"
    }
    $tag = "v$version"

    if (git status --porcelain) { Fail 'commit or stash your changes first' }
    git fetch --quiet --tags origin
    if ($LASTEXITCODE -ne 0) { Fail "can't reach origin" }
    if ((git rev-list --count '@{u}..HEAD') -ne '0') { Fail 'push first' }
    if (git tag --list $tag) { Fail "$tag already exists" }

    # The version's notes: the lines under its heading, up to the next.
    $lines = Get-Content 'CHANGELOG.md'
    $start = ($lines | Select-String -Pattern "^## \[$([regex]::Escape($version))\]" |
        Select-Object -First 1).LineNumber
    if (-not $start) { Fail "CHANGELOG.md has no '## [$version]' section" }
    $notes = @()
    foreach ($line in $lines[$start..($lines.Count - 1)]) {
        if ($line -match '^## \[' -or $line -match '^\[.+\]: ') { break }
        $notes += $line
    }
    $notes = ($notes -join "`n").Trim()
    if (-not $notes) { Fail "the $version section of CHANGELOG.md is empty" }

    Write-Host "== $tag"
    Write-Host $notes
    if ($DryRun) {
        Write-Host '== dry run: nothing tagged or published'
        return
    }

    powershell -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'package.ps1')
    if ($LASTEXITCODE -ne 0) { Fail 'packaging failed' }
    $zip = Join-Path $root "dist\wallflex-$version.zip"
    if (-not (Test-Path $zip)) { Fail "$zip is missing" }

    git tag -a $tag -m "Wallflex $version"
    git push origin $tag
    if ($LASTEXITCODE -ne 0) { Fail 'pushing the tag failed' }

    $notesFile = Join-Path ([IO.Path]::GetTempPath()) "wallflex-$tag-notes.md"
    $setup = "Download **wallflex-$version.zip**, unzip it somewhere " +
        "permanent and see SETUP.md inside.`n`n"
    Set-Content -Path $notesFile -Value ($setup + $notes) -Encoding utf8
    & $gh release create $tag $zip --title "Wallflex $version" --notes-file $notesFile
    if ($LASTEXITCODE -ne 0) { Fail 'creating the GitHub release failed' }
    Remove-Item $notesFile
    Write-Host "== released $tag"
}
finally {
    Pop-Location
}
