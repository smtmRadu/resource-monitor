# Isolated LibreHardwareMonitor reader: CPU package temperature + power.
# Writes {"temp":..,"watts":..,"ts":..} to -Out every 2s (atomic replace).
# Elevated: loads LibreHardwareMonitorLib directly (needs admin for its driver).
# Otherwise: reads the LHM app's WMI namespace when that app is running.
param([Parameter(Mandatory = $true)][string]$Out)
$ErrorActionPreference = 'SilentlyContinue'

$dll = Join-Path $PSScriptRoot 'LHM\LibreHardwareMonitorLib.dll'
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
$computer = $null
if ($admin -and (Test-Path $dll)) {
    try {
        Add-Type -Path $dll
        $computer = New-Object LibreHardwareMonitor.Hardware.Computer
        $computer.IsCpuEnabled = $true
        $computer.Open()
    } catch { $computer = $null }
}

# Intel names the die sensor "CPU Package"; AMD reports "Core (Tctl/Tdie)".
function Pick($sensors) {
    $t = $null; $w = $null
    foreach ($s in $sensors) {
        if ($null -eq $s.Value) { continue }
        $type = "$($s.SensorType)"; $name = "$($s.Name)"
        if ($type -eq 'Temperature' -and $null -eq $t -and ($name -like '*Package*' -or $name -like '*Tctl*' -or $name -like '*Tdie*')) { $t = [double]$s.Value }
        elseif ($type -eq 'Power' -and $null -eq $w -and $name -like '*Package*') { $w = [double]$s.Value }
    }
    if ($null -eq $t) {
        $cores = @($sensors | Where-Object { "$($_.SensorType)" -eq 'Temperature' -and $null -ne $_.Value -and "$($_.Name)" -like '*Core*' })
        if ($cores.Count) { $t = [double](($cores | Measure-Object -Property Value -Maximum).Maximum) }
    }
    return @($t, $w)
}

while ($true) {
    $t = $null; $w = $null
    if ($computer) {
        $all = @()
        foreach ($hw in $computer.Hardware) {
            try { $hw.Update() } catch { continue }
            $all += $hw.Sensors
        }
        $t, $w = Pick $all
    } else {
        $wmi = Get-CimInstance -Namespace 'root/LibreHardwareMonitor' -ClassName Sensor -ErrorAction SilentlyContinue
        if ($wmi) { $t, $w = Pick ($wmi | Where-Object { "$($_.Parent)" -like '*cpu*' -or "$($_.Identifier)" -like '*cpu*' }) }
        elseif (-not $admin) { exit 0 }  # nothing to read; the monitor respawns later if LHM starts
    }
    $ts = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() / 1000.0
    $json = '{"temp":' + $(if ($null -eq $t) { 'null' } else { $t.ToString([Globalization.CultureInfo]::InvariantCulture) }) +
            ',"watts":' + $(if ($null -eq $w) { 'null' } else { $w.ToString([Globalization.CultureInfo]::InvariantCulture) }) +
            ',"ts":' + $ts.ToString([Globalization.CultureInfo]::InvariantCulture) + '}'
    try {
        [IO.File]::WriteAllText("$Out.tmp", $json)
        Move-Item -Force "$Out.tmp" $Out
    } catch {}
    Start-Sleep -Seconds 2
}
