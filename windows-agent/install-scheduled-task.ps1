param(
    [string]$TaskName = "M5StackPcRemoteAgent",
    [string]$ExePath = "$PSScriptRoot\target\release\pc-remote-agent.exe",
    [string]$ConfigPath = "$PSScriptRoot\config.toml"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $ExePath)) {
    throw "Agent executable not found: $ExePath"
}

if (-not (Test-Path $ConfigPath)) {
    throw "Config file not found: $ConfigPath"
}

$action = New-ScheduledTaskAction `
    -Execute $ExePath `
    -Argument "--config `"$ConfigPath`""

$trigger = New-ScheduledTaskTrigger -AtStartup
$principal = New-ScheduledTaskPrincipal `
    -UserId "SYSTEM" `
    -RunLevel Highest

Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $action `
    -Trigger $trigger `
    -Principal $principal `
    -Description "Runs the m5stack-pc-remote Windows Agent at startup." `
    -Force

Write-Host "Scheduled task installed: $TaskName"
