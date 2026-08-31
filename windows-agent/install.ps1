# Installs the m5stack-pc-remote Windows Agent as a startup Scheduled Task.
# Must be run from an elevated (Administrator) PowerShell.
param(
    [string]$InstallDir = "$env:ProgramData\m5stack-pc-remote-agent",
    [string]$ExePath = "$PSScriptRoot\target\x86_64-pc-windows-gnu\release\pc-remote-agent.exe",
    [string]$ConfigPath = "$PSScriptRoot\config.toml",
    [string]$TaskName = "M5StackPcRemoteAgent",
    [switch]$Start
)

$ErrorActionPreference = "Stop"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)) {
    throw "Administrator権限のPowerShellで実行してください。"
}

if (-not (Test-Path $ExePath)) {
    $fallback = "$PSScriptRoot\target\release\pc-remote-agent.exe"
    if (Test-Path $fallback) {
        $ExePath = $fallback
    } else {
        throw "Agent executable not found: $ExePath`n先にビルドしてください (Windows上: cargo build --release / WSL等からのクロスビルド: cargo build --release --target x86_64-pc-windows-gnu)。"
    }
}

if (-not (Test-Path $ConfigPath)) {
    $examplePath = "$PSScriptRoot\config.example.toml"
    if (-not (Test-Path $examplePath)) {
        throw "Config file not found: $ConfigPath"
    }
    Write-Host "config.toml が見つからないため config.example.toml から生成します。"
    $secretChars = (48..57) + (65..90) + (97..122)
    $secret = -join ((1..48) | ForEach-Object { [char]($secretChars | Get-Random) })
    (Get-Content $examplePath) -replace 'replace-with-a-long-random-shared-secret', $secret |
        Set-Content $ConfigPath
    Write-Host "shared_secret を新規生成しました。firmware/include/config.h の AGENT_SHARED_SECRET を同じ値に必ず合わせてください。"
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Path $ExePath -Destination "$InstallDir\pc-remote-agent.exe" -Force
Copy-Item -Path $ConfigPath -Destination "$InstallDir\config.toml" -Force

$configContent = Get-Content "$InstallDir\config.toml" -Raw
$port = "18080"
if ($configContent -match 'bind\s*=\s*"[^:]+:(\d+)"') {
    $port = $Matches[1]
}

$firewallRuleName = "m5stack-pc-remote-agent inbound $port"
if (-not (Get-NetFirewallRule -DisplayName $firewallRuleName -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule `
        -DisplayName $firewallRuleName `
        -Direction Inbound `
        -Action Allow `
        -Protocol TCP `
        -LocalPort $port `
        -Profile Private | Out-Null
    Write-Host "Firewall rule created: $firewallRuleName (Private profile only)"
} else {
    Write-Host "Firewall rule already exists: $firewallRuleName"
}

$action = New-ScheduledTaskAction `
    -Execute "$InstallDir\pc-remote-agent.exe" `
    -Argument "--config `"$InstallDir\config.toml`""
$trigger = New-ScheduledTaskTrigger -AtStartup
$taskPrincipal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -RunLevel Highest

Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $action `
    -Trigger $trigger `
    -Principal $taskPrincipal `
    -Description "Runs the m5stack-pc-remote Windows Agent at startup." `
    -Force | Out-Null

Write-Host "Scheduled task installed: $TaskName"
Write-Host "Install dir: $InstallDir"

if ($Start) {
    Start-ScheduledTask -TaskName $TaskName
    Write-Host "Scheduled task started."
}

Write-Host ""
Write-Host "dry_run = true の間は実際のshutdown/rebootは実行されません。"
Write-Host "本番運用前に $InstallDir\config.toml の dry_run を確認してください。"
