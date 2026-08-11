# TuwaiqOS Product D0 — Windows entry point
# Builds the bootable disk image using Docker (Linux builder container).
[CmdletBinding()]
param(
    [switch]$SkipSmoke
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Need-Command([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Missing dependency: $Name"
    }
}

Write-Host "[build-product] repo=$Root"
Need-Command docker

try {
    docker info | Out-Null
} catch {
    throw "Docker is not running. Start Docker Desktop, then re-run."
}

Write-Host "[build-product] building builder image"
docker build -t tuwaiqos-product-builder:d0 -f product/build/Dockerfile product/build
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

New-Item -ItemType Directory -Force -Path (Join-Path $Root "product\out") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Root "product\build\work") | Out-Null

# Convert Windows path to Docker Desktop mount form when needed
$src = ($Root -replace '\\','/')
if ($src -match '^[A-Za-z]:') {
    $drive = $src.Substring(0,1).ToLower()
    $src = "/${drive}" + $src.Substring(2)
}

Write-Host "[build-product] running privileged disk build (long)"
docker run --rm --privileged -v "${src}:/src" -w /src tuwaiqos-product-builder:d0 bash product/build/build-rootfs-disk.sh
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "[build-product] artifacts:"
Get-ChildItem (Join-Path $Root "product\out") -ErrorAction SilentlyContinue | Format-Table Name, Length

if (-not $SkipSmoke) {
    $smoke = Join-Path $PSScriptRoot "product-desktop-smoke.ps1"
    if (Test-Path $smoke) {
        Write-Host "[build-product] invoking smoke harness"
        & powershell -File $smoke -AllowMissingImage
    }
}

Write-Host "[build-product] done"
