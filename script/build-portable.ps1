# SPDX-License-Identifier: AGPL-3.0-only
# Modelled on warp-terminal `script/windows/build-portable.ps1` and `package-release.ps1`
# @ 5bb7bcc6a (build with the release tag embedded, verify the linked binary, stage and zip);
# the script is new.

<#
.SYNOPSIS
    Builds the portable MatterLite archive for Windows x64 (MatterLite.md §16).

.DESCRIPTION
    1. Builds `matterlite` with the `rlto` profile (thin LTO, symbols stripped) and
       GIT_RELEASE_TAG set, so the version is compiled in (log, window title).
    2. Verifies the linked binary: the tag is in it (the archive name can't lie), it is a GUI
       program (PE subsystem 2: no console window), and it is large enough to hold the
       embedded assets (fonts, themes, the emoji images).
    3. Stages matterlite.exe, an empty `data` folder (which makes it portable) with a
       README, and the licences; checks that no user data got into `data`.
    4. Writes dist\matterlite-<tag>-windows-x64.zip and prints its SHA-256.

.PARAMETER Tag
    The release tag, `v<semver>` (e.g. v0.3.0). Default: the crate version as a development
    build of this commit, e.g. v0.1.0-dev.1a2b3c4.

.PARAMETER OutDir
    Where the staging folder and the archive go. Default: dist.

.EXAMPLE
    .\script\build-portable.ps1

.EXAMPLE
    .\script\build-portable.ps1 -Tag v0.3.0
#>

Param(
    [String]$Tag,
    [String]$OutDir = 'dist'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Platform = 'windows-x64'
# The emoji images alone are 6.4 MB; a binary without the embedded assets is far smaller.
$MinBinarySize = 12MB

Push-Location $RepoRoot
try {
    if (-not $Tag) {
        $version = (Select-String -Path 'crates\matterlite\Cargo.toml' -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
        $commit = (git rev-parse --short=7 HEAD).Trim()
        $Tag = "v$version-dev.$commit"
    }
    if ($Tag -notmatch '^v\d+\.\d+\.\d+([-+][0-9A-Za-z.+-]+)?$') {
        throw "Tag '$Tag' isn't v<semver> (e.g. v0.3.0 or v0.3.0-rc.1)."
    }
    Write-Host "== MatterLite $Tag for $Platform"

    $env:GIT_RELEASE_TAG = $Tag
    cargo build --profile rlto --locked -p matterlite
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    $exe = Join-Path $RepoRoot 'target\rlto\matterlite.exe'

    # The checks.
    $bytes = [IO.File]::ReadAllBytes($exe)
    $text = [Text.Encoding]::GetEncoding(28591).GetString($bytes)   # Latin-1: one char per byte
    if (-not $text.Contains($Tag)) {
        throw "The built binary doesn't contain '$Tag': it wasn't rebuilt with the tag."
    }
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
    $subsystem = [BitConverter]::ToUInt16($bytes, $peOffset + 0x5C)
    if ($subsystem -ne 2) {
        throw "PE subsystem is $subsystem, not 2 (GUI): the release build would open a console."
    }
    if ($bytes.Length -lt $MinBinarySize) {
        throw "The binary is only $($bytes.Length) bytes: the assets aren't embedded."
    }
    Write-Host "   verified: version $Tag, GUI subsystem, $([math]::Round($bytes.Length / 1MB, 1)) MB"

    # Stage.
    $name = "matterlite-$Tag-$Platform"
    New-Item -ItemType Directory -Force $OutDir | Out-Null
    $stage = Join-Path $OutDir $name
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force (Join-Path $stage 'data') | Out-Null
    Copy-Item $exe $stage
    Set-Content -Encoding utf8 -Path (Join-Path $stage 'data\README.txt') -Value @(
        'MatterLite keeps its settings, cache and logs in this folder (portable mode).',
        'Delete the folder to use the per-user folders of your system instead.'
    )
    foreach ($file in 'LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE', 'README.md', 'PROVENANCE.md') {
        Copy-Item $file $stage
    }

    # No user data from test runs in the archive.
    $extra = Get-ChildItem -Recurse -Force (Join-Path $stage 'data') |
        Where-Object { $_.Name -ne 'README.txt' }
    if ($extra) { throw "Unexpected files in the staged data folder: $($extra.FullName -join ', ')" }

    # Archive.
    $archive = Join-Path $OutDir "$name.zip"
    if (Test-Path $archive) { Remove-Item -Force $archive }
    Compress-Archive -Path $stage -DestinationPath $archive
    $hash = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLower()
    Write-Host "== $archive"
    Write-Host "   sha256 $hash"
}
finally {
    Pop-Location
}
