# One-time setup: start Resource Monitor elevated at every logon, without a UAC prompt.
# Elevation enables CPU temp/package watts (LibreHardwareMonitor driver) and FPS (PresentMon).
# Run from an elevated PowerShell. Paths come from this script's folder (no hardcoded user dirs).
$exe = Join-Path $PSScriptRoot 'target\release\resource-monitor-rs.exe'
if (-not (Test-Path $exe)) { throw "Build first: cargo build --release ($exe not found)" }
schtasks /create /tn "ResourceMonitorRs" /tr "`"$exe`"" /sc onlogon /delay 0000:15 /rl highest /f
schtasks /run /tn "ResourceMonitorRs"
