# Removes the m5stack-pc-remote Windows Agent Scheduled Task, firewall rule,
# and (optionally) the installed files.
# Must be run from an elevated (Administrator) PowerShell.
param(
    [string]$InstallDir = "$env:ProgramData\m5stack-pc-remote-agent",
    [string]$TaskName = "M5StackPcRemoteAgent",
    [switch]$RemoveFiles
)

$ErrorActionPreference = "Stop"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)) {
    throw "Administrator権限のPowerShellで実行してください。"
}

if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    Write-Host "Scheduled task removed: $TaskName"
} else {
    Write-Host "Scheduled task not found: $TaskName"
}

$rules = Get-NetFirewallRule -DisplayName "m5stack-pc-remote-agent inbound *" -ErrorAction SilentlyContinue
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
    Write-Host "Install directory kept: $InstallDir (-RemoveFiles で削除できます。config.tomlにshared_secretが含まれ、ACLはAdministratorsとSYSTEMのみ読み取り可能に制限されています。手動でACLを変更していないか確認してください)"
}
