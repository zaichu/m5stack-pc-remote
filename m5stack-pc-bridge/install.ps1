# Installs m5stack-pc-bridge as a Windows Service (SCM管理、自動起動、異常終了時は自動再起動)。
# Must be run from an elevated (Administrator) PowerShell.
param(
    [string]$InstallDir = "$env:ProgramData\m5stack-pc-bridge",
    [string]$ExePath = "$PSScriptRoot\target\x86_64-pc-windows-gnu\release\m5stack-pc-bridge.exe",
    [string]$ConfigPath = "$PSScriptRoot\config.toml",
    [string]$ServiceName = "M5StackPcBridge",
    [switch]$Start
)

$ErrorActionPreference = "Stop"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)) {
    throw "Administrator権限のPowerShellで実行してください。"
}

if (-not (Test-Path $ExePath)) {
    $fallback = "$PSScriptRoot\target\release\m5stack-pc-bridge.exe"
    if (Test-Path $fallback) {
        $ExePath = $fallback
    } else {
        throw "m5stack-pc-bridge executable not found: $ExePath`n先にビルドしてください (Windows上: cargo build --release / WSL等からのクロスビルド: cargo build --release --target x86_64-pc-windows-gnu)。"
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

# 旧版(Task Schedulerで常駐)からの移行: 同じポートを両方が使おうとすると起動に失敗するため、
# 新しいServiceを登録する前に旧タスクを止めて消しておく。
$legacyTaskName = "M5StackPcRemoteAgent"
if (Get-ScheduledTask -TaskName $legacyTaskName -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $legacyTaskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $legacyTaskName -Confirm:$false
    Write-Host "旧Scheduled Taskを削除しました: $legacyTaskName"
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

$installedExePath = "$InstallDir\m5stack-pc-bridge.exe"

# Serviceが実行中だとexeを上書きできないため、更新の場合は先に止める。
if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
    Stop-Service -Name $ServiceName -ErrorAction SilentlyContinue
}

Copy-Item -Path $ExePath -Destination $installedExePath -Force
# 設定ファイルの既定パスは「実行ファイルと同じディレクトリのconfig.toml」なので、
# ここでインストール先へ確定させる(Service起動時はCWDが %SystemRoot%\System32 になるため)。
Copy-Item -Path $ConfigPath -Destination "$InstallDir\config.toml" -Force

$configContent = Get-Content "$InstallDir\config.toml" -Raw
$port = "18080"
if ($configContent -match 'bind\s*=\s*"[^:]+:(\d+)"') {
    $port = $Matches[1]
}

$firewallRuleName = "m5stack-pc-bridge inbound $port"
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

$binaryPathName = "`"$installedExePath`""
$existingService = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existingService) {
    sc.exe config $ServiceName binPath= $binaryPathName start= auto | Out-Null
    Write-Host "既存のServiceを更新しました: $ServiceName"
} else {
    New-Service `
        -Name $ServiceName `
        -BinaryPathName $binaryPathName `
        -DisplayName "M5Stack PC Bridge" `
        -Description "M5Stack Core2からのHMAC署名付きリクエストを受け、Windows PCのreboot/shutdownを実行します。" `
        -StartupType Automatic | Out-Null
    Write-Host "Serviceを登録しました: $ServiceName"
}

# 異常終了時は自動再起動する(New-Service/Set-Serviceにrecovery設定のcmdletが無いためsc.exeを使う)。
# reset=86400: 24時間無事故が続いたら失敗カウントを0に戻す。
sc.exe failure $ServiceName reset= 86400 actions= restart/5000/restart/5000/restart/5000 | Out-Null

Write-Host "Service: $ServiceName"
Write-Host "Install dir: $InstallDir"

if ($Start) {
    Start-Service -Name $ServiceName
    Write-Host "Service started."
}

Write-Host ""
Write-Host "dry_run = true の間は実際のshutdown/rebootは実行されません。"
Write-Host "本番運用前に $InstallDir\config.toml の dry_run を確認してください。"
