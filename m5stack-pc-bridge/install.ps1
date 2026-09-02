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

# このスクリプトをWSL上のリポジトリ(\\wsl.localhost\...)から実行すると、カレント
# ディレクトリがUNCパスになる。icacls/sc.exeのような古いコンソールツールは、
# プロセスの作業ディレクトリがUNCパスだと内部的なワイルドカード解決(/Tなど)で
# 「アクセスが拒否されました」を返すことがある。スクリプト自身のパス解決は
# すべて $PSScriptRoot(絶対パス)基準なので、カレントディレクトリをローカルパスへ
# 変えても他の処理に影響しない。
Set-Location -Path $env:SystemRoot

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

    # Get-Content/Set-Contentの既定文字コードは環境依存で、Windows PowerShell 5.1では
    # BOM無しUTF-8をシステムのANSIコードページ(日本語Windowsだと大抵Shift-JIS)として
    # 誤読することがある。config.example.tomlの日本語コメントが文字化けし、生成される
    # config.tomlがTOMLとして壊れて読み込めなくなる(Serviceが起動直後に落ちる)ため、
    # 読み込みはUTF-8を明示し、書き込みは.NETのWriteAllTextでBOM無しUTF-8を直接指定する。
    # (Windows PowerShell 5.1の Set-Content -Encoding UTF8 は常にBOMを付けてしまい、
    # RustのtomlパーサーがBOM付きファイルを解釈できないため。)
    $templateContent = Get-Content -Path $examplePath -Raw -Encoding UTF8
    $generatedContent = $templateContent -replace 'replace-with-a-long-random-shared-secret', $secret
    [System.IO.File]::WriteAllText($ConfigPath, $generatedContent, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "shared_secret を暗号論的乱数(64文字)で新規生成しました。firmware/config.toml の bridge_shared_secret を同じ値に必ず合わせてください。"
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

$installedExePath = "$InstallDir\m5stack-pc-bridge.exe"

# Serviceが実行中だとexeを上書きできないため、更新の場合は先に止める。
# Stop-Serviceはstatus=Stoppedになるまでブロックするが、プロセス側がexeファイル
# ハンドルを完全に解放するまで短い遅延が入ることがあるため、WaitForStatusで
# 明示的に待ち、Copy-Itemもリトライして "使用中" による失敗を吸収する。
$existingServiceBeforeInstall = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existingServiceBeforeInstall) {
    Stop-Service -Name $ServiceName -ErrorAction SilentlyContinue
    try {
        $existingServiceBeforeInstall.WaitForStatus('Stopped', (New-TimeSpan -Seconds 15))
    } catch {
        Write-Host "WARNING: Serviceの停止待ちがタイムアウトしました。処理を続行します。"
    }
}

# SCM経由でない直接実行(開発時のforegroundフォールバック)や、前回の停止漏れで
# m5stack-pc-bridge.exeがまだ起動したままの場合、上書きコピーが失敗するため
# 名前が一致するプロセスを先に止めておく。
Get-Process -Name "m5stack-pc-bridge" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

# 以前はAdministrators/SYSTEM限定にACLをロックダウンしていたが、環境によって
# icaclsの/grantが「成功」と報告しつつ実際には適用されず、誰もアクセスできない
# 空のACLになって復旧できなくなる事象が起きたため、ロックダウンはやめ、
# %ProgramData%からの既定の継承ACLをそのまま使う方針にした(判断: 2026-09-03)。
# config.tomlのshared_secretは、このPCの他のローカルアカウントからも読める状態に
# なる(このPCを他ユーザーと共有していない前提)。
# 既存ファイルが上記の壊れたACLのまま残っている場合に備え、上書き前に継承を
# 明示的に復元しておく。所有者(Administrators)はDACLが空でもWRITE_DACを常に
# 持つため、この呼び出し自体が失敗することはない。
if (Test-Path $installedExePath) {
    icacls $installedExePath /reset | Out-Null
}

$copyExeAttempts = 5
for ($i = 1; $i -le $copyExeAttempts; $i++) {
    try {
        Copy-Item -Path $ExePath -Destination $installedExePath -Force
        break
    } catch {
        if ($i -eq $copyExeAttempts) {
            Write-Host "実行ファイルのコピーに失敗しました。現在のACL/所有者:"
            icacls $installedExePath
            throw
        }
        Write-Host "実行ファイルが使用中のため再試行します ($i/$copyExeAttempts)..."
        Start-Sleep -Milliseconds 500
    }
}
# 設定ファイルの既定パスは「実行ファイルと同じディレクトリのconfig.toml」なので、
# ここでインストール先へ確定させる(Service起動時はCWDが %SystemRoot%\System32 になるため)。
# exeと同様、既存ファイルの壊れたACLに備えて上書き前に継承を復元しておく。
$installedConfigPath = "$InstallDir\config.toml"
if (Test-Path $installedConfigPath) {
    icacls $installedConfigPath /reset | Out-Null
}
Copy-Item -Path $ConfigPath -Destination $installedConfigPath -Force

$configContent = Get-Content $ConfigPath -Raw
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
