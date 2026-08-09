# Build a self-contained BIOS/NVMe ESXi artifact from the normal raw image.

[CmdletBinding()]
param(
    [ValidateRange(13, 21)]
    [int]$VirtualHardwareVersion = 13,
    [string]$OutputDirectory,
    [string]$QemuImg,
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ProjectRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $ProjectRoot 'target\esxi'
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$AllowedOutputRoot = [IO.Path]::GetFullPath((Join-Path $ProjectRoot 'target'))
if (-not $OutputDirectory.StartsWith($AllowedOutputRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "ESXi output must remain below '$AllowedOutputRoot'."
}

if (-not $QemuImg) {
    $command = Get-Command qemu-img -ErrorAction SilentlyContinue
    if ($command) {
        $QemuImg = $command.Source
    } else {
        $QemuImg = Join-Path $env:ProgramFiles 'qemu\qemu-img.exe'
    }
}
$QemuImg = [IO.Path]::GetFullPath($QemuImg)
if (-not (Test-Path -LiteralPath $QemuImg -PathType Leaf)) {
    throw "qemu-img was not found. Pass -QemuImg with the installed executable path."
}

# Query this installed binary rather than assuming its VMDK implementation.
$VmdkOptions = (& $QemuImg create -f vmdk -o help 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0) { throw "qemu-img could not enumerate VMDK options." }
foreach ($required in @('hwversion=', 'adapter_type=', 'subformat=')) {
    if ($VmdkOptions -notmatch [regex]::Escape($required)) {
        throw "Installed qemu-img lacks required VMDK option '$required'."
    }
}

if (-not $SkipBuild) {
    & (Join-Path $PSScriptRoot 'build.ps1')
    if ($LASTEXITCODE -ne 0) { throw 'Normal TuwaiqOS build failed.' }
}

$RawImage = Join-Path $ProjectRoot 'target\debug\boot-bios-tuwaiqos.img'
if (-not (Test-Path -LiteralPath $RawImage -PathType Leaf)) {
    throw "Raw BIOS image is missing at '$RawImage'."
}
$RawHashBefore = (Get-FileHash -Algorithm SHA256 -LiteralPath $RawImage).Hash
$RawLength = (Get-Item -LiteralPath $RawImage).Length

[void](New-Item -ItemType Directory -Force -Path $OutputDirectory)
$Vmdk = Join-Path $OutputDirectory 'TuwaiqOS-ESXi.vmdk'
$Vmx = Join-Path $OutputDirectory 'TuwaiqOS-ESXi.vmx'
$Checksums = Join-Path $OutputDirectory 'SHA256SUMS.txt'
$Readme = Join-Path $OutputDirectory 'README-ESXi.md'
foreach ($artifact in @($Vmdk, $Vmx, $Checksums, $Readme)) {
    if (Test-Path -LiteralPath $artifact) {
        Remove-Item -LiteralPath $artifact -Force
    }
}

$CreateOptions = "subformat=streamOptimized,adapter_type=lsilogic,hwversion=$VirtualHardwareVersion"
& $QemuImg convert -f raw -O vmdk -o $CreateOptions $RawImage $Vmdk
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $Vmdk -PathType Leaf)) {
    throw 'qemu-img VMDK conversion failed.'
}

$Info = (& $QemuImg info --output=json $Vmdk | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0 -or $Info.format -ne 'vmdk' -or [int64]$Info.'virtual-size' -ne $RawLength) {
    throw 'qemu-img info did not validate the generated VMDK geometry.'
}
$Check = (& $QemuImg check --output=json $Vmdk | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0 -or [int64]$Check.'check-errors' -ne 0) {
    throw 'qemu-img check found VMDK errors.'
}

$VmxText = @"
.encoding = "UTF-8"
config.version = "8"
virtualHW.version = "$VirtualHardwareVersion"
displayName = "TuwaiqOS Phase 7B"
guestOS = "other-64"
firmware = "bios"
numvcpus = "1"
memSize = "512"
cpuid.coresPerSocket = "1"
bios.bootOrder = "hdd"
nvme0.present = "TRUE"
nvme0:0.present = "TRUE"
nvme0:0.fileName = "TuwaiqOS-ESXi.vmdk"
nvme0:0.redo = ""
ethernet0.present = "FALSE"
usb.present = "TRUE"
usb_xhci.present = "FALSE"
floppy0.present = "FALSE"
serial0.present = "TRUE"
serial0.fileType = "file"
serial0.fileName = "TuwaiqOS-ESXi-COM1.log"
serial0.yieldOnMsrRead = "TRUE"
mks.enable3d = "FALSE"
"@
[IO.File]::WriteAllText($Vmx, $VmxText, [Text.UTF8Encoding]::new($false))

$ReadmeTemplate = Get-Content -LiteralPath (Join-Path $ProjectRoot 'docs\ESXI.md') -Raw
$ReadmeText = $ReadmeTemplate.Replace('{{VIRTUAL_HW_VERSION}}', [string]$VirtualHardwareVersion)
[IO.File]::WriteAllText($Readme, $ReadmeText, [Text.UTF8Encoding]::new($false))

$RawHashAfter = (Get-FileHash -Algorithm SHA256 -LiteralPath $RawImage).Hash
if ($RawHashAfter -ne $RawHashBefore) {
    throw 'The source raw image changed during ESXi conversion.'
}

$ChecksumLines = foreach ($artifact in @($Vmdk, $Vmx, $Readme)) {
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $artifact).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($artifact))"
}
$ChecksumLines += "$($RawHashAfter.ToLowerInvariant())  source/boot-bios-tuwaiqos.img"
[IO.File]::WriteAllLines($Checksums, $ChecksumLines, [Text.UTF8Encoding]::new($false))

Write-Host 'ESXi artifact PASS' -ForegroundColor Green
Write-Host "  VMDK: $Vmdk"
Write-Host "  VMX:  $Vmx"
Write-Host "  SHA:  $Checksums"
Write-Host "  README: $Readme"
Write-Host "  qemu-img: $($Info.format), virtual-size=$($Info.'virtual-size'), check-errors=$($Check.'check-errors')"
Write-Host "  source raw unchanged: $RawHashAfter"
