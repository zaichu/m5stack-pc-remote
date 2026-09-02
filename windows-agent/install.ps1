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
    # URL-safe base64相当の64文字集合を使う。256は64の倍数なので & 0x3F の変換に剰余バイアスが出ない。
    $secretAlphabet = [char[]](
        [char[]]([char]'A'..[char]'Z') +
        [char[]]([char]'a'..[char]'z') +
        [char[]]([char]'0'..[char]'9') +
        @('-', '_')
    )
    $secretLength = 64
    $randomBytes = New-Object byte[] $secretLength
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $rng.GetBytes($randomBytes)
    } finally {
        $rng.Dispose()
    }
    $secret = -join ($randomBytes | ForEach-Object { $secretAlphabet[$_ -band 0x3F] })
    (Get-Content $examplePath) -replace 'replace-with-a-long-random-shared-secret', $secret |
        Set-Content $ConfigPath
    Write-Host "shared_secret を暗号論的乱数(64文字)で新規生成しました。firmware/config.toml の agent_shared_secret を同じ値に必ず合わせてください。"
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

# config.toml に shared_secret が含まれるため、インストール先ディレクトリのACLを
# Administrators (S-1-5-32-544) と SYSTEM (S-1-5-18) のみに制限する。
# SIDを使うのはローカライズされたグループ名(例: 独語版Windowsの"Administratoren")に
# 依存しないようにするため。/T で既存の子ファイル(config.toml, exe)にも再帰適用する。
icacls $InstallDir /inheritance:r /T | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "ACLの継承解除に失敗しました: $InstallDir"
}
icacls $InstallDir /grant:r "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-18:(OI)(CI)F" /T | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "ACLの設定に失敗しました: $InstallDir"
}
Write-Host "インストール先ディレクトリのACLをAdministratorsとSYSTEMのみに制限しました: $InstallDir"

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
