# Build a release into dist/: the exe and manifest.json that installed copies update from.
# Bump `version` in Cargo.toml first, run this, then commit dist/ and push to main to publish.
#   .\release.ps1              exe + manifest
#   .\release.ps1 -Installer   also rebuild dist\ResourceMonitor-Setup.exe (only when
#                              installer\setup.iss changes: it downloads the app itself)
param([switch]$Installer)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

cargo build --release
if ($LASTEXITCODE) { throw 'cargo build failed' }
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\resource-monitor-rs.exe dist\ -Force

# path = where it goes in the install folder; src = repo path to download it from.
function Entry([string]$Src, [string]$Path) {
    [ordered]@{ path = $Path; src = $Src; sha256 = (Get-FileHash $Src -Algorithm SHA256).Hash.ToLower() }
}
$files = @(
    (Entry 'dist/resource-monitor-rs.exe' 'resource-monitor-rs.exe'),
    (Entry 'setup_task.ps1' 'setup_task.ps1'),
    (Entry 'vendor/lhm_bridge.ps1' 'vendor/lhm_bridge.ps1')
)
# Sensor helpers; debug symbols and API docs are not needed at runtime.
$files += Get-ChildItem vendor\PresentMon, vendor\LHM -Recurse -File |
    # LibreHardwareMonitor.config: LHM rewrites it at runtime, so its hash never matches.
    Where-Object { $_.Extension -notin '.pdb', '.xml' -and $_.Name -ne 'LibreHardwareMonitor.config' } |
    ForEach-Object {
        $rel = $_.FullName.Substring($PSScriptRoot.Length + 1).Replace('\', '/')
        Entry $rel $rel
    }
$json = [ordered]@{ version = $version; files = $files } | ConvertTo-Json -Depth 4
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'dist\manifest.json'), $json, (New-Object Text.UTF8Encoding $false))

if ($Installer) {
    $iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $iscc) { throw 'Inno Setup 6 not found: winget install JRSoftware.InnoSetup' }
    & $iscc /Q installer\setup.iss
    if ($LASTEXITCODE) { throw 'installer build failed' }
}
Write-Host "dist/ ready: v$version, $($files.Count) files. Commit dist/ and push to main to publish."
