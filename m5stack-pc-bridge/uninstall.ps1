# Removes the m5stack-pc-bridge Windows Service, firewall rule, and (optionally)
# the installed files.
# Must be run from an elevated (Administrator) PowerShell.
param(
    [string]$InstallDir = "$env:ProgramData\m5stack-pc-bridge",
    [string]$ServiceName = "M5StackPcBridge",
    [switch]$RemoveFiles
)

$ErrorActionPreference = "Stop"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)) {
    throw "Administrator権限のPowerShellで実行してください。"
}

# install.ps1と同じ理由(UNCパスのカレントディレクトリがsc.exe等の古いコンソール
# ツールで問題を起こすことがある)で、ローカルパスへ変えておく。
Set-Location -Path $env:SystemRoot

if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
    Stop-Service -Name $ServiceName -ErrorAction SilentlyContinue
    # Remove-ServiceはPowerShell 6+限定のため、Windows PowerShell 5.1でも動くsc.exeを使う。
    sc.exe delete $ServiceName | Out-Null
    Write-Host "Service removed: $ServiceName"
} else {
    Write-Host "Service not found: $ServiceName"
}

# 旧版(Task Scheduler常駐)が残っていれば合わせて削除する。
$legacyTaskName = "M5StackPcRemoteAgent"
if (Get-ScheduledTask -TaskName $legacyTaskName -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $legacyTaskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $legacyTaskName -Confirm:$false
    Write-Host "旧Scheduled Taskを削除しました: $legacyTaskName"
}

$rules = Get-NetFirewallRule -DisplayName "m5stack-pc-bridge inbound *" -ErrorAction SilentlyContinue
if ($rules) {
    foreach ($rule in $rules) {
        Remove-NetFirewallRule -Name $rule.Name
        Write-Host "Firewall rule removed: $($rule.DisplayName)"
    }
} else {
    Write-Host "Firewall rule not found."
}

if ($RemoveFiles) {
    if (Test-Path $InstallDir) {
        Remove-Item -Recurse -Force $InstallDir
        Write-Host "Install directory removed: $InstallDir"
    }
} else {
    Write-Host "Install directory kept: $InstallDir (-RemoveFiles で削除できます。config.tomlにshared_secretが含まれます)"
}
