# Boot TuwaiqOS D1 qcow2 with graphical display + virtio-net + serial log (Windows host)
# Preserves D0 stack: IDE/AHCI disk + virtio-vga + Plasma X11.
[CmdletBinding()]
param(
    [int]$Seconds = 240,
    [string]$Image,
    [ValidateSet("sdl","gtk","vnc")]
    [string]$Display = "sdl",
    [int]$MonitorPort = 4451,
    [switch]$UserNet
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
if (-not $Image) {
    $proof = Join-Path $Root "product\out\d1-proof.qcow2"
    $canon = Join-Path $Root "product\out\tuwaiqos-d1.qcow2"
    $d0proof = "C:\Users\asdks\Projects\TuwaiqOS\target\worktrees\product-desktop-foundation\product\out\d0-proof.qcow2"
    if (Test-Path $proof) { $Image = $proof }
    elseif (Test-Path $canon) { $Image = $canon }
    elseif (Test-Path $d0proof) { $Image = $d0proof }
}
if (-not $Image -or -not (Test-Path $Image)) { throw "Image not found: $Image" }

$freeMB = [math]::Round((Get-PSDrive C).Free / 1MB, 1)
if ($freeMB -lt 512) {
    Write-Warning "Low free space on C: (${freeMB} MB). Guest may fail if host cannot allocate RAM backing."
}

$qemu = "C:\Program Files\qemu\qemu-system-x86_64.exe"
if (-not (Test-Path $qemu)) { throw "QEMU not found: $qemu" }

$OutDir = Join-Path $Root "product\out\smoke"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$serial = Join-Path $OutDir "boot-serial-d1.log"
if (Test-Path $serial) { Remove-Item $serial -Force }

$displayArgs = switch ($Display) {
    "sdl" { @("-display", "sdl") }
    "gtk" { @("-display", "gtk") }
    "vnc" { @("-vnc", ":1") }
}

# D0-proven disk/display + D1 virtio-net (user networking for QEMU acceptance)
$argList = @(
    "-machine", "q35",
    "-m", "2048",
    "-smp", "2",
    "-drive", "file=$Image,format=qcow2,if=none,id=disk0",
    "-device", "ich9-ahci,id=ahci",
    "-device", "ide-hd,drive=disk0,bus=ahci.0",
    "-device", "virtio-vga",
    "-netdev", "user,id=net0",
    "-device", "virtio-net-pci,netdev=net0",
    "-usb",
    "-device", "usb-tablet",
    "-serial", "file:$serial",
    "-monitor", "tcp:127.0.0.1:$MonitorPort,server,nowait",
    "-no-reboot"
) + $displayArgs

Write-Host "[boot-product-gui] image=$Image"
Write-Host "[boot-product-gui] nic=virtio-net-pci netdev=user"
Write-Host "[boot-product-gui] command: $qemu $($argList -join ' ')"
Write-Host "[boot-product-gui] serial=$serial (wait ${Seconds}s)"

$proc = Start-Process -FilePath $qemu -ArgumentList $argList -PassThru
$shots = New-Object System.Collections.Generic.List[string]
$prev = 0
foreach ($t in @(20, 45, 75, 110, 150, 190, 230)) {
    if ($t -ge $Seconds) { break }
    $wait = $t - $prev
    if ($wait -gt 0) { Start-Sleep -Seconds $wait }
    $prev = $t
    if ($proc.HasExited) {
        Write-Host "[boot-product-gui] qemu exited early code=$($proc.ExitCode)"
        break
    }
    $png = Join-Path $OutDir ("d1-screendump-{0:D3}s.ppm" -f $t)
    try {
        $client = New-Object System.Net.Sockets.TcpClient
        $client.Connect("127.0.0.1", $MonitorPort)
        $stream = $client.GetStream()
        $writer = New-Object System.IO.StreamWriter($stream)
        $writer.NewLine = "`n"
        $writer.AutoFlush = $true
        $reader = New-Object System.IO.StreamReader($stream)
        Start-Sleep -Milliseconds 300
        while ($stream.DataAvailable) { [void]$reader.Read() }
        $ppmPath = ($png -replace '\\','/')
        $writer.WriteLine("screendump $ppmPath")
        Start-Sleep -Milliseconds 700
        while ($stream.DataAvailable) { [void]$reader.Read() }
        $client.Close()
        if (Test-Path $png) {
            $shots.Add($png) | Out-Null
            Write-Host "[boot-product-gui] screendump t=${t}s size=$((Get-Item $png).Length)"
        } else {
            Write-Host "[boot-product-gui] screendump t=${t}s missing"
        }
    } catch {
        Write-Host "[boot-product-gui] screendump t=${t}s error: $($_.Exception.Message)"
    }
}

$remain = $Seconds - $prev
if ($remain -gt 0 -and -not $proc.HasExited) {
    Start-Sleep -Seconds $remain
}

if (-not $proc.HasExited) {
    Write-Host "[boot-product-gui] stopping guest after ${Seconds}s"
    try {
        $client = New-Object System.Net.Sockets.TcpClient
        $client.Connect("127.0.0.1", $MonitorPort)
        $stream = $client.GetStream()
        $writer = New-Object System.IO.StreamWriter($stream)
        $writer.NewLine = "`n"
        $writer.AutoFlush = $true
        $final = Join-Path $OutDir "d1-screendump-final.ppm"
        $ppmPath = ($final -replace '\\','/')
        $writer.WriteLine("screendump $ppmPath")
        Start-Sleep -Milliseconds 800
        $writer.WriteLine("quit")
        Start-Sleep -Seconds 1
        $client.Close()
        if (Test-Path $final) { $shots.Add($final) | Out-Null }
    } catch {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Seconds 2
    if (-not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}

if (Test-Path $serial) {
    Write-Host "[boot-product-gui] serial bytes=$((Get-Item $serial).Length)"
    Write-Host "==== SERIAL TAIL ===="
    Get-Content $serial -Tail 120 -ErrorAction SilentlyContinue
} else {
    Write-Host "[boot-product-gui] no serial log produced"
}

Write-Host "[boot-product-gui] screendumps=$($shots.Count)"
$shots | ForEach-Object { Write-Host "  $_" }
