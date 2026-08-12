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

# Linux-backed volume for debootstrap/rootfs (Windows bind mounts break dpkg extract).
Write-Host "[build-product] ensuring Docker volume tuwaiqos-d0-work"
docker volume create tuwaiqos-d0-work | Out-Null

Write-Host "[build-product] running privileged disk build (long)"
docker run --rm --privileged `
  -v "${src}:/src" `
  -v "tuwaiqos-d0-work:/work" `
  -e "TUWAIQ_WORK=/work" `
  -e "TUWAIQ_DEBOOTSTRAP_RETRIES=3" `
  -e "TUWAIQ_RESUME=auto" `
  -e "TUWAIQ_UBUNTU_MIRROR=http://azure.archive.ubuntu.com/ubuntu" `
  -w /src `
  tuwaiqos-product-builder:d0 `
  bash product/build/build-rootfs-disk.sh
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "[build-product] exporting disk image from docker volume"
$outDir = Join-Path $Root "product\out"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$qcow = Join-Path $outDir "tuwaiqos-d0.qcow2"
$raw = Join-Path $outDir "tuwaiqos-d0.raw"

# Prefer qcow2 (sparse) — large raw copies to Windows bind mounts often hit I/O errors.
docker run --rm -v "tuwaiqos-d0-work:/work" -v "${src}:/src" tuwaiqos-product-builder:d0 bash -lc `
  "test -f /work/tuwaiqos-d0.qcow2 && cp -f /work/tuwaiqos-d0.qcow2 /src/product/out/tuwaiqos-d0.qcow2 || `
   (test -f /work/tuwaiqos-d0.raw && dd if=/work/tuwaiqos-d0.raw of=/src/product/out/tuwaiqos-d0.raw bs=64M conv=fsync status=progress)"
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
