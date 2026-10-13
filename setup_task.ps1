# Start Resource Monitor elevated at every logon of the current user, without a UAC prompt.
# Elevation enables CPU temp/package watts (LibreHardwareMonitor driver) and FPS (PresentMon).
# Run from an elevated PowerShell. The installer runs it for "Enable on startup".
#   .\setup_task.ps1                 dev build (target\release), starts it now
#   .\setup_task.ps1 -Exe <path>     an installed copy
#   .\setup_task.ps1 -Remove         delete the task
param([string]$Exe = (Join-Path $PSScriptRoot 'target\release\resource-monitor-rs.exe'), [switch]$Remove)
$ErrorActionPreference = 'Stop'
$name = 'ResourceMonitorRs'
Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
if ($Remove) { exit 0 }
if (-not (Test-Path $Exe)) { throw "Not found: $Exe (dev: cargo build --release first)" }

$user = "$env:USERDOMAIN\$env:USERNAME"
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$trigger.Delay = 'PT15S'  # let the shell (tray) come up first
# schtasks defaults would skip the task on battery, kill it on unplug and after 3 days.
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $name -Force `
    -Action (New-ScheduledTaskAction -Execute $Exe -WorkingDirectory (Split-Path $Exe)) `
    -Trigger $trigger -Settings $settings `
    -Principal (New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest) | Out-Null
Start-ScheduledTask -TaskName $name
