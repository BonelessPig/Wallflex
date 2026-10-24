<#
.SYNOPSIS
Measures Wallflex's memory and CPU use.

.DESCRIPTION
Starts Wallflex, waits for it to settle, samples it for -Seconds, then
stops it with --quit.

Working set is the RAM Task Manager's "Memory" column is closest to;
private bytes is memory only this process uses. CPU is averaged over the
sample, as a share of one core and of the whole CPU. "GPU driver" says
whether the NVIDIA, AMD or Intel user-mode driver was loaded.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File tools\measure.ps1 -Seconds 60
#>
param(
    [int]$Seconds = 30,
    [int]$SettleSeconds = 5,
    # Settings file to run with, instead of the user's own.
    [string]$Config = '',
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\wallflex.exe')
)

$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path $Exe).Path
# User-mode graphics driver DLLs, by vendor.
$driverPattern = '^(nvwgf2um|nvd3dum|nvldumd|atiumd|amdxx|igdumd|igd10|igdgmm)'
$cores = [Environment]::ProcessorCount

if (Get-Process -Name 'wallflex' -ErrorAction SilentlyContinue) {
    & $Exe --quit | Out-Null
    Start-Sleep -Seconds 2
}

$arguments = @()
if ($Config) { $arguments = @('--config', $Config) }
$proc = if ($arguments) {
    Start-Process -FilePath $Exe -ArgumentList $arguments -PassThru
} else {
    Start-Process -FilePath $Exe -PassThru
}
Start-Sleep -Seconds $SettleSeconds
$proc.Refresh()
$cpuStart = $proc.TotalProcessorTime.TotalSeconds
$wsPeak = 0
$privatePeak = 0
for ($i = 0; $i -lt $Seconds; $i++) {
    Start-Sleep -Seconds 1
    $proc.Refresh()
    $wsPeak = [math]::Max($wsPeak, $proc.WorkingSet64)
    $privatePeak = [math]::Max($privatePeak, $proc.PrivateMemorySize64)
}
$cpuUsed = $proc.TotalProcessorTime.TotalSeconds - $cpuStart
$driver = $proc.Modules | Where-Object { $_.ModuleName -match $driverPattern }
$row = [pscustomobject]@{
    'Working set MB' = [math]::Round($proc.WorkingSet64 / 1MB, 1)
    'Peak WS MB'     = [math]::Round($wsPeak / 1MB, 1)
    'Private MB'     = [math]::Round($proc.PrivateMemorySize64 / 1MB, 1)
    'Peak private'   = [math]::Round($privatePeak / 1MB, 1)
    'CPU % of core'  = [math]::Round($cpuUsed / $Seconds * 100, 3)
    'CPU % of all'   = [math]::Round($cpuUsed / $Seconds / $cores * 100, 4)
    Threads          = $proc.Threads.Count
    DLLs             = $proc.Modules.Count
    'GPU driver'     = if ($driver) { 'yes' } else { 'no' }
}
& $Exe --quit | Out-Null
$proc.WaitForExit(5000) | Out-Null

$size = [math]::Round((Get-Item $Exe).Length / 1KB)
"wallflex.exe: $size KB; sampled $Seconds s on $cores threads"
$row | Format-Table -AutoSize
