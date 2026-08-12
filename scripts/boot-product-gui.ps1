# Boot TuwaiqOS D0 qcow2 with graphical display + serial log (Windows host)
[CmdletBinding()]
param(
    [int]$Seconds = 240,
    [string]$Image,
    [ValidateSet("sdl","gtk","vnc")]
    [string]$Display = "sdl"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
if (-not $Image) {
    $proof = Join-Path $Root "product\out\d0-proof.qcow2"
    $canon = Join-Path $Root "product\out\tuwaiqos-d0.qcow2"
    $Image = if (Test-Path $proof) { $proof } else { $canon }
}
if (-not (Test-Path $Image)) { throw "Image not found: $Image" }

$qemu = "C:\Program Files\qemu\qemu-system-x86_64.exe"
if (-not (Test-Path $qemu)) { throw "QEMU not found: $qemu" }

$OutDir = Join-Path $Root "product\out\smoke"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$serial = Join-Path $OutDir "boot-serial-gui.log"
if (Test-Path $serial) { Remove-Item $serial -Force }

$displayArgs = switch ($Display) {
    "sdl" { @("-display", "sdl") }
    "gtk" { @("-display", "gtk") }
    "vnc" { @("-vnc", ":1") }
}

# IDE disk for SeaBIOS+GRUB reliability; virtio-vga for guest DRM (linux-image-virtual
# has virtio-gpu but not bochs). USB tablet for mouse.
$argList = @(
    "-machine", "q35",
    "-m", "3072",
    "-smp", "2",
    "-drive", "file=$Image,format=qcow2,if=none,id=disk0",
    "-device", "ich9-ahci,id=ahci",
    "-device", "ide-hd,drive=disk0,bus=ahci.0",
    "-device", "virtio-vga",
    "-usb",
    "-device", "usb-tablet",
    "-serial", "file:$serial",
    "-monitor", "tcp:127.0.0.1:4444,server,nowait",
    "-no-reboot"
) + $displayArgs

Write-Host "[boot-product-gui] image=$Image"
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
    $png = Join-Path $OutDir ("screendump-{0:D3}s.ppm" -f $t)
    try {
        $client = New-Object System.Net.Sockets.TcpClient
        $client.Connect("127.0.0.1", 4444)
        $stream = $client.GetStream()
        $writer = New-Object System.IO.StreamWriter($stream)
        $writer.NewLine = "`n"
        $writer.AutoFlush = $true
        $reader = New-Object System.IO.StreamReader($stream)
        Start-Sleep -Milliseconds 300
        while ($stream.DataAvailable) { [void]$reader.Read() }
        # QEMU on Windows wants forward slashes or escaped paths
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
        $client.Connect("127.0.0.1", 4444)
        $stream = $client.GetStream()
        $writer = New-Object System.IO.StreamWriter($stream)
        $writer.NewLine = "`n"
        $writer.AutoFlush = $true
        $final = Join-Path $OutDir "screendump-final.ppm"
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
    Get-Content $serial -Tail 100 -ErrorAction SilentlyContinue
} else {
    Write-Host "[boot-product-gui] no serial log produced"
}

Write-Host "[boot-product-gui] screendumps=$($shots.Count)"
$shots | ForEach-Object { Write-Host "  $_" }
