# Focused Phase 6 mutable-storage and reboot-persistence smoke test.

[CmdletBinding()]
param(
    [string]$Image,
    [string]$OutDir,
    [int]$MonitorPort = 45961,
    [int]$SerialPort = 45962,
    [switch]$AllowDirty
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $ProjectRoot
if (-not $Image) { $Image = Join-Path $ProjectRoot "target\debug\boot-bios-tuwaiqos.img" }
$Image = [System.IO.Path]::GetFullPath($Image)
if (-not (Test-Path -LiteralPath $Image -PathType Leaf)) {
    throw "Disk image not found at '$Image'. Run scripts\build.ps1 first."
}

$Dirty = @(& git status --porcelain=v1 --untracked-files=all)
if ($Dirty.Count -gt 0 -and -not $AllowDirty) {
    throw "Worktree is dirty; commit the candidate or pass -AllowDirty for development."
}
$Commit = (& git rev-parse HEAD).Trim()
$Timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
if (-not $OutDir) {
    $OutDir = Join-Path $ProjectRoot "target\phase6-storage-smoke\$($Commit.Substring(0, 12))-$Timestamp"
}
$OutDir = [System.IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$TestImage = Join-Path $OutDir "storage-smoke.img"
Copy-Item -LiteralPath $Image -Destination $TestImage

$QemuCommand = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
$Qemu = if ($QemuCommand) { $QemuCommand.Source } else { "C:\Program Files\qemu\qemu-system-x86_64.exe" }
if (-not (Test-Path -LiteralPath $Qemu -PathType Leaf)) { throw "qemu-system-x86_64 not found." }

$SerialLog = Join-Path $OutDir "serial.log"
$ResultsJson = Join-Path $OutDir "results.json"
$ManifestJson = Join-Path $OutDir "manifest.json"
$SerialText = [System.Text.StringBuilder]::new()
$Results = [System.Collections.Generic.List[object]]::new()
$Proc = $null
$MonitorClient = $null
$SerialClient = $null
$MonitorStream = $null
$SerialStream = $null
$MonitorWriter = $null

function Pump-Serial {
    param([int]$WaitMilliseconds = 0)
    if ($WaitMilliseconds -gt 0) { Start-Sleep -Milliseconds $WaitMilliseconds }
    while ($null -ne $SerialStream -and $SerialStream.DataAvailable) {
        $buffer = New-Object byte[] 16384
        $count = $SerialStream.Read($buffer, 0, $buffer.Length)
        if ($count -le 0) { break }
        [void]$SerialText.Append([System.Text.Encoding]::ASCII.GetString($buffer, 0, $count))
    }
}

function Save-Evidence {
    Pump-Serial
    [System.IO.File]::WriteAllText($SerialLog, $SerialText.ToString(), [System.Text.Encoding]::UTF8)
    $Results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $ResultsJson -Encoding UTF8
    [pscustomobject]@{
        schema = 1
        git_commit = $Commit
        source_image = $Image
        source_image_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $Image).Hash.ToLowerInvariant()
        tested_image = $TestImage
        qemu = $Qemu
        verdict = if ($Results.Count -eq 6) { "PASS" } else { "INCOMPLETE" }
    } | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $ManifestJson -Encoding UTF8
}

function Test-Health {
    $all = $SerialText.ToString()
    if ($all -match 'KERNEL PANIC|kernel page fault|EXCEPTION: DOUBLE FAULT') {
        throw "Kernel panic/fault detected."
    }
    if ($null -ne $Proc -and $Proc.HasExited) { throw "QEMU exited unexpectedly." }
}

function Connect-Tcp {
    param([int]$Port, [string]$Name)
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline) {
        $client = [System.Net.Sockets.TcpClient]::new()
        try { $client.Connect("127.0.0.1", $Port); return $client } catch {
            $client.Dispose()
            Start-Sleep -Milliseconds 200
        }
    }
    throw "Timed out connecting to $Name on port $Port."
}

function Invoke-Monitor {
    param([string]$Command, [int]$SettleMilliseconds = 35)
    $MonitorWriter.WriteLine($Command)
    Start-Sleep -Milliseconds $SettleMilliseconds
    while ($MonitorStream.DataAvailable) {
        $discard = New-Object byte[] 4096
        [void]$MonitorStream.Read($discard, 0, $discard.Length)
    }
    Pump-Serial
    Test-Health
}

function Send-Text {
    param([string]$Text)
    $map = @{ ' ' = 'spc'; '-' = 'minus'; '.' = 'dot'; '/' = 'slash' }
    foreach ($character in $Text.ToCharArray()) {
        $value = [string]$character
        if ($map.ContainsKey($value)) { Invoke-Monitor "sendkey $($map[$value])" }
        elseif ($value -cmatch '^[a-z0-9]$') { Invoke-Monitor "sendkey $value" }
        else { throw "No sendkey mapping for '$value'." }
    }
}

function Wait-Regex {
    param([string]$Pattern, [int]$Offset = 0, [int]$TimeoutSeconds = 45)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        Pump-Serial 100
        Test-Health
        $all = $SerialText.ToString()
        if ($Offset -gt $all.Length) { $Offset = $all.Length }
        if ($all.Substring($Offset) -match $Pattern) { return $Matches[0] }
    }
    Save-Evidence
    throw "Timed out waiting for '$Pattern'."
}

function Add-Pass {
    param([string]$Name, [string]$Evidence)
    $Results.Add([pscustomobject]@{ name = $Name; status = "PASS"; evidence = $Evidence })
    Write-Host "[PASS] $Name - $Evidence" -ForegroundColor Green
}

function Invoke-ShellCommand {
    param([string]$Command, [string[]]$Expected = @(), [int]$TimeoutSeconds = 60)
    Pump-Serial
    $offset = $SerialText.Length
    Send-Text $Command
    Invoke-Monitor "sendkey ret"
    [void](Wait-Regex "shell: command complete: $([regex]::Escape($Command))(?:\r?\n|$)" $offset $TimeoutSeconds)
    Pump-Serial 100
    $segment = $SerialText.ToString().Substring($offset)
    foreach ($pattern in $Expected) {
        if ($segment -notmatch $pattern) { throw "'$Command' missed expected '$pattern'." }
    }
    Test-Health
    return $segment
}

try {
    $arguments = @(
        "-drive", "format=raw,file=$TestImage", "-m", "128M", "-display", "none",
        "-serial", "tcp:127.0.0.1:$SerialPort,server,nowait",
        "-monitor", "tcp:127.0.0.1:$MonitorPort,server,nowait", "-no-shutdown"
    )
    $Proc = Start-Process -FilePath $Qemu -ArgumentList $arguments -PassThru -WindowStyle Hidden
    $SerialClient = Connect-Tcp $SerialPort "serial"
    $MonitorClient = Connect-Tcp $MonitorPort "monitor"
    $SerialStream = $SerialClient.GetStream()
    $MonitorStream = $MonitorClient.GetStream()
    $MonitorWriter = [System.IO.StreamWriter]::new($MonitorStream)
    $MonitorWriter.AutoFlush = $true

    [void](Wait-Regex 'task heartbeat: beat #1(?:\r?\n|$)' 0 90)
    [void](Wait-Regex 'vfs: mounted TuwaiqFS v2 at /' 0 5)
    Add-Pass "boot and VFS mount" "kernel reached scheduler with TuwaiqFS mounted"

    $segment = Invoke-ShellCommand "storagetest" @('file-mutation: PASS', 'storage: PASS') 120
    Add-Pass "mutable Ring-3 storage ABI" ([regex]::Match($segment, 'file-mutation: PASS[^\r\n]*').Value)

    [void](Invoke-ShellCommand "notes set reboot-note durable-storage" @('Saved note: reboot-note'))
    [void](Invoke-ShellCommand "notes show reboot-note" @('durable-storage'))
    Add-Pass "built-in Notes save/reopen" "Notes saved and reopened data through VFS before reboot"

    [void](Invoke-ShellCommand "installapp hello" @('/apps/hello'))
    [void](Invoke-ShellCommand "runfs /apps/hello" @('hello: Ring 3 ELF process alive', 'exit_code=0'))
    Add-Pass "filesystem-backed ELF" "installed executable loaded through VFS and exited 0"

    Pump-Serial
    $rebootOffset = $SerialText.Length
    Send-Text "reboot"
    Invoke-Monitor "sendkey ret"
    [void](Wait-Regex 'TuwaiqOS v0\.5 kernel_main: booting' $rebootOffset 60)
    [void](Wait-Regex 'task heartbeat: beat #1(?:\r?\n|$)' $rebootOffset 90)
    [void](Invoke-ShellCommand "cat /data/file-mutation-test/persist.txt" @('ring3-persist-v2'))
    [void](Invoke-ShellCommand "notes show reboot-note" @('durable-storage'))
    Add-Pass "genuine reboot persistence" "Ring-3 file and built-in Notes data survived reboot"

    [void](Invoke-ShellCommand "runfs /apps/hello" @('hello: Ring 3 ELF process alive', 'exit_code=0'))
    Test-Health
    Add-Pass "post-reboot ELF and kernel health" "filesystem ELF relaunched; no panic, page fault, or double fault"

    Save-Evidence
    $MonitorWriter.WriteLine("quit")
    Start-Sleep -Milliseconds 300
    Write-Host "Phase 6 storage smoke PASS: $OutDir" -ForegroundColor Green
} finally {
    try { Save-Evidence } catch {}
    if ($null -ne $MonitorWriter) { try { $MonitorWriter.WriteLine("quit") } catch {} }
    foreach ($resource in @($MonitorWriter, $MonitorStream, $SerialStream, $MonitorClient, $SerialClient)) {
        if ($null -ne $resource) { try { $resource.Dispose() } catch {} }
    }
    if ($null -ne $Proc -and -not $Proc.HasExited) {
        Stop-Process -Id $Proc.Id -Force -ErrorAction SilentlyContinue
    }
}
