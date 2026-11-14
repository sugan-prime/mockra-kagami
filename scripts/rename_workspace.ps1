# Mockra Kagami Workspace Physical Directory Migration Script
# Run this script from an external PowerShell window when the IDE / Antigravity is closed.

$ErrorActionPreference = "Stop"
$Parent = "C:\Developer\Github"
$OldDir = Join-Path $Parent "switch-emulator"
$NewDir = Join-Path $Parent "mockra-kagami"

Write-Host "Migrating workspace physical directory to mockra-kagami..." -ForegroundColor Cyan

if (Test-Path $NewDir) {
    $item = Get-Item $NewDir
    if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
        Write-Host "Removing temporary junction $NewDir..."
        Remove-Item $NewDir -Force
    }
}

if (Test-Path $OldDir) {
    Write-Host "Renaming $OldDir -> mockra-kagami..."
    Rename-Item -Path $OldDir -NewName "mockra-kagami"
    Write-Host "Creating backward-compatible junction $OldDir -> $NewDir..."
    New-Item -ItemType Junction -Path $OldDir -Target $NewDir
    Write-Host "`nWorkspace successfully migrated! Physical directory is now: $NewDir" -ForegroundColor Green
} else {
    Write-Host "$OldDir does not exist; check paths." -ForegroundColor Yellow
}
