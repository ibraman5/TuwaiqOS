# Tuwaiq Desktop D0 product smoke harness (Windows host)
[CmdletBinding()]
param(
    [string]$Image,
    [switch]$AllowMissingImage,
    [int]$BootSeconds = 90
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
if (-not $Image) {
    $qcow2 = Join-Path $Root "product\out\tuwaiqos-d0.qcow2"
    $raw = Join-Path $Root "product\out\tuwaiqos-d0.raw"
    if (Test-Path $qcow2) { $Image = $qcow2 } else { $Image = $raw }
}

$OutDir = Join-Path $Root "product\out\smoke"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$Results = New-Object System.Collections.Generic.List[object]

function Add-Result([string]$Name, [string]$Status, [string]$Evidence) {
    $Results.Add([pscustomobject]@{ name = $Name; status = $Status; evidence = $Evidence })
    Write-Host ("[{0}] {1}: {2}" -f $Status, $Name, $Evidence)
}

function Find-Qemu {
    $c = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    $p = "C:\Program Files\qemu\qemu-system-x86_64.exe"
    if (Test-Path $p) { return $p }
    return $null
}

# Static / packaging checks (do not require boot)
$brandSvg = Join-Path $Root "product\branding\wallpapers\TuwaiqOS\contents\images\1920x1080.svg"
$osConf = Join-Path $Root "product\config\tuwaiqos.conf"
$adr = Join-Path $Root "docs\adr\0001-product-linux-foundation.md"
if ((Test-Path $brandSvg) -and (Test-Path $osConf)) {
    Add-Result "TUWAIQ_BRANDING" "PASS" "branding assets + tuwaiqos.conf present"
} else {
    Add-Result "TUWAIQ_BRANDING" "FAIL" "branding files missing"
}

if (Test-Path $adr) {
    Add-Result "ADR_PRESENT" "PASS" "docs/adr/0001-product-linux-foundation.md"
} else {
    Add-Result "ADR_PRESENT" "FAIL" "ADR missing"
}

$third = Join-Path $Root "docs\product\THIRD_PARTY.md"
if (Test-Path $third) {
    Add-Result "LICENSING_DOC" "PASS" "docs/product/THIRD_PARTY.md"
} else {
    Add-Result "LICENSING_DOC" "FAIL" "third-party doc missing"
}

if (-not (Test-Path -LiteralPath $Image)) {
    if ($AllowMissingImage) {
        foreach ($n in @("PRODUCT_BOOT","DISPLAY_MANAGER","WAYLAND_SESSION","DESKTOP_READY","LAUNCHER_READY","TERMINAL_READY","FILE_MANAGER_READY","INPUT_READY","NETWORK_STATE","SHUTDOWN_READY")) {
            Add-Result $n "SKIP" "image not built yet: $Image"
        }
        $overall = "PARTIAL"
        $Results | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $OutDir "results.json")
        Write-Host "OVERALL: $overall"
        exit 0
    }
    throw "Image not found: $Image"
}

Add-Result "IMAGE_PRESENT" "PASS" $Image
$qemu = Find-Qemu
if (-not $qemu) {
    foreach ($n in @("PRODUCT_BOOT","DISPLAY_MANAGER","WAYLAND_SESSION","DESKTOP_READY","LAUNCHER_READY","TERMINAL_READY","FILE_MANAGER_READY","INPUT_READY","NETWORK_STATE","SHUTDOWN_READY")) {
        Add-Result $n "SKIP" "qemu-system-x86_64 not found"
    }
    $Results | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $OutDir "results.json")
    Write-Host "OVERALL: PARTIAL"
    exit 0
}

$serial = Join-Path $OutDir "boot-serial.log"
if (Test-Path $serial) { Remove-Item $serial -Force }
$driveFmt = if ($Image -match '\.qcow2$') { 'qcow2' } else { 'raw' }
if ($driveFmt -ne 'qcow2') {
    Add-Result "IMAGE_FORMAT" "FAIL" "D0 smoke requires qcow2 (got $Image)"
} else {
    Add-Result "IMAGE_FORMAT" "PASS" "qcow2"
}

$qemuImg = "C:\Program Files\qemu\qemu-img.exe"
if (Test-Path $qemuImg) {
    $check = & $qemuImg check $Image 2>&1 | Out-String
    if ($check -match 'No errors were found') {
        Add-Result "QCOW2_CHECK" "PASS" "qemu-img check clean"
    } else {
        Add-Result "QCOW2_CHECK" "FAIL" ($check.Trim())
    }
}

$qemuArgs = @(
    "-machine", "q35",
    "-m", "2048",
    "-smp", "2",
    "-drive", "format=$driveFmt,file=$Image,if=virtio",
    "-display", "none",
    "-serial", "file:$serial",
    "-no-reboot"
)

# Attempt BIOS boot; UEFI may need OVMF which is optional
Write-Host "[smoke] booting image for ${BootSeconds}s"
$proc = Start-Process -FilePath $qemu -ArgumentList $qemuArgs -PassThru -WindowStyle Hidden
Start-Sleep -Seconds $BootSeconds
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

$serialText = ""
if (Test-Path $serial) {
    try { $serialText = [IO.File]::ReadAllText($serial) } catch { $serialText = "" }
}

$grub = $serialText -match 'GNU GRUB|TuwaiqOS D0'
$login = $serialText -match 'login:'
$multi = $serialText -match 'TUWAIQ_D0_MULTI_USER|Reached target Multi-User|multi-user\.target'
$graph = $serialText -match 'TUWAIQ_D0_GRAPHICAL_EVIDENCE|Reached target Graphical|graphical\.target'
$sddm = $serialText -match '(?i)sddm'
$plasma = $serialText -match '(?i)plasma'

if ($grub -and ($login -or $multi -or ($serialText -match 'Linux version|systemd'))) {
    Add-Result "PRODUCT_BOOT" "PASS" "SeaBIOS/GRUB + userspace serial markers"
} elseif ($serialText.Length -gt 0) {
    Add-Result "PRODUCT_BOOT" "PARTIAL" "serial non-empty but incomplete boot markers"
} else {
    Add-Result "PRODUCT_BOOT" "FAIL" "empty serial (likely needs BIOS/GRUB path)"
}

Add-Result "GRUB_MARKER" $(if ($grub) { "PASS" } else { "FAIL" }) $(if ($grub) { "GRUB/TuwaiqOS D0 seen" } else { "missing" })
Add-Result "LOGIN_OR_MULTIUSER" $(if ($login -or $multi) { "PASS" } else { "PARTIAL" }) $(if ($login) { "login prompt" } elseif ($multi) { "multi-user marker" } else { "not seen" })

if ($graph -or $sddm) {
    Add-Result "DISPLAY_MANAGER" "PASS" $(if ($sddm) { "sddm serial marker" } else { "graphical evidence marker" })
} else {
    Add-Result "DISPLAY_MANAGER" "SKIP" "no SDDM/graphical serial marker; use scripts/boot-product-gui.ps1"
}

# Remaining GUI proofs still require interactive/display evidence.
foreach ($n in @("WAYLAND_SESSION","DESKTOP_READY","LAUNCHER_READY","TERMINAL_READY","FILE_MANAGER_READY","INPUT_READY","NETWORK_STATE","SHUTDOWN_READY")) {
    Add-Result $n "SKIP" "no automated GUI evidence in headless serial smoke; verify with boot-product-gui.ps1"
}

$fail = @($Results | Where-Object { $_.status -eq "FAIL" }).Count
$passBoot = ($Results | Where-Object { $_.name -eq "PRODUCT_BOOT" }).status
$overall = if ($fail -gt 0) { "FAIL" } elseif ($passBoot -eq "PASS") { "PARTIAL" } else { "PARTIAL" }

$Results | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $OutDir "results.json")
Write-Host "OVERALL: $overall"
Write-Host "Evidence: $OutDir"
if ($serialText.Length -gt 0) {
    Write-Host ("Serial bytes: {0}" -f $serialText.Length)
}
