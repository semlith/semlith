<#
.SYNOPSIS
    Install semlith from a GitHub release: download the Windows binary, verify
    it against SHA256SUMS, install it into ~\.semlith\bin and hand off to
    `semlith setup`.

        irm https://raw.githubusercontent.com/semlith/semlith/main/install.ps1 | iex
.DESCRIPTION
    SEMLITH_VERSION  pin a release tag such as v0.10.0 (default: latest)
    SEMLITH_HOME     install into <dir>\bin (default: ~\.semlith\bin)
    SEMLITH_YES      set to 1 to answer yes to every `semlith setup` prompt
#>
$ErrorActionPreference = 'Stop'

$repo = 'semlith/semlith'
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne 'AMD64' -and $arch -ne 'ARM64') {
    throw "semlith has no prebuilt Windows binary for $arch."
}
# One Windows target is built; on ARM64 it runs under Windows 11's x64 emulation.
if ($arch -eq 'ARM64') {
    Write-Information 'No native ARM64 build yet; installing the x64 binary, which Windows emulates.' -InformationAction Continue
}
$target = 'x86_64-pc-windows-msvc'

if ($env:SEMLITH_VERSION) {
    $tag = $env:SEMLITH_VERSION
}
else {
    Write-Information 'Resolving the latest semlith release...' -InformationAction Continue
    $latest = Invoke-WebRequest -Uri "https://github.com/$repo/releases/latest" -UseBasicParsing
    $response = $latest.BaseResponse
    if ($response.ResponseUri) {
        $resolved = $response.ResponseUri.AbsoluteUri
    }
    else {
        $resolved = $response.RequestMessage.RequestUri.AbsoluteUri
    }
    $tag = $resolved.Split('/')[-1]
}
if ($tag -notmatch '^v') {
    throw "could not resolve a release tag (got '$tag')"
}

$name = "semlith-$tag-$target"
$archive = "$name.zip"
$base = "https://github.com/$repo/releases/download/$tag"
Write-Information "Installing semlith $tag for $target" -InformationAction Continue

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('semlith-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $archivePath = Join-Path $tmp $archive
    $sumsPath = Join-Path $tmp 'SHA256SUMS'

    Write-Information "Downloading $archive" -InformationAction Continue
    Invoke-WebRequest -Uri "$base/$archive" -OutFile $archivePath -UseBasicParsing
    Write-Information 'Downloading SHA256SUMS' -InformationAction Continue
    Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sumsPath -UseBasicParsing

    Write-Information 'Verifying checksum' -InformationAction Continue
    $pattern = '\s' + [regex]::Escape($archive) + '$'
    $line = Get-Content -Path $sumsPath | Where-Object { $_ -match $pattern } | Select-Object -First 1
    if (-not $line) {
        throw "SHA256SUMS has no entry for $archive"
    }
    $expected = ($line -split '\s+')[0]
    $actual = (Get-FileHash -Path $archivePath -Algorithm SHA256).Hash
    if ($actual -ne $expected) {
        throw "checksum mismatch: $archive hashes to $actual but SHA256SUMS says $expected; nothing was installed"
    }

    Write-Information 'Unpacking' -InformationAction Continue
    Expand-Archive -Path $archivePath -DestinationPath $tmp -Force
    $exe = Join-Path (Join-Path $tmp $name) 'semlith.exe'
    Unblock-File -Path $exe

    if ($env:SEMLITH_HOME) {
        $binDir = Join-Path $env:SEMLITH_HOME 'bin'
    }
    else {
        $binDir = Join-Path $HOME '.semlith\bin'
    }
    New-Item -ItemType Directory -Path $binDir -Force | Out-Null
    $installed = Join-Path $binDir 'semlith.exe'
    Move-Item -Path $exe -Destination $installed -Force
    Write-Information "Installed $installed" -InformationAction Continue
}
finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

$hasSetup = $false
try {
    & $installed setup --help *> $null
    $hasSetup = ($LASTEXITCODE -eq 0)
}
catch {
    $hasSetup = $false
}

if ($hasSetup) {
    if ($env:SEMLITH_YES -eq '1') {
        & $installed setup --yes
    }
    else {
        & $installed setup
    }
}
else {
    Write-Information '' -InformationAction Continue
    Write-Information "This release has no 'setup' command, so add semlith to your PATH:" -InformationAction Continue
    Write-Information "    `$env:Path = '$binDir;' + `$env:Path" -InformationAction Continue
    Write-Information 'To make that permanent, run:' -InformationAction Continue
    Write-Information "    setx PATH `"$binDir;%PATH%`"" -InformationAction Continue
}
