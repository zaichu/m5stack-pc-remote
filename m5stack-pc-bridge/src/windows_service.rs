//! Windows Service(SCM)としての起動。`cfg(windows)`専用で、Linux上のビルド/テストには
//! 一切影響しない(Cargo.tomlで`windows-service` crateをcfg(windows)依存にしているため)。
//!
//! `install.ps1` が `New-Service` で登録するサービス名(`SERVICE_NAME`)と一致している
//! 必要がある。

use std::ffi::OsString;
use std::sync::mpsc;
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use crate::app_config::AgentConfig;
use crate::server;

pub const SERVICE_NAME: &str = "M5StackPcBridge";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

/// SCMがサービスを起動していないプロセスから`StartServiceCtrlDispatcher`を呼んだ場合の
/// Win32エラーコード(`ERROR_FAILED_SERVICE_CONTROLLER_CONNECT`)。値はWindows APIの
/// 固定値で、`windows`/`windows-sys` crateへの直接依存を増やすほどではないためハードコードする。
const ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: i32 = 1063;

/// SCM経由なら`service_main`を、開発時にexeを直接実行した場合はforegroundで動かす。
pub fn run() -> anyhow::Result<()> {
    match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => Ok(()),
        Err(windows_service::Error::Winapi(io_err))
            if io_err.raw_os_error() == Some(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT) =>
        {
            eprintln!(
                "Service Control Managerからの起動ではないため、foregroundで実行します(動作確認用)。"
            );
            run_foreground()
        }
        Err(e) => Err(anyhow::anyhow!("failed to start service dispatcher: {e}")),
    }
}

fn run_foreground() -> anyhow::Result<()> {
    let config_path = crate::default_config_path();
    let config = AgentConfig::from_path(&config_path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {e}", config_path.display()))?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(server::serve(config))
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    // service_mainはWindows側の規約でResultを返せない。失敗はプロセスの異常終了として
    // SCMへ伝わり、install.ps1が設定するrecovery(自動再起動)に任せる。
    if let Err(e) = run_service() {
        // Windows Serviceにはコンソールが無く、eprintln!はどこにも表示されない。
        // 実行ファイルと同じディレクトリのログファイルへ書き、起動失敗の原因を
        // 追えるようにする(secretは書かない: エラーメッセージにconfig.tomlの値は含まれない)。
        log_startup_error(&e);
    }
}

fn log_startup_error(err: &anyhow::Error) {
    use std::io::Write;

    let log_path = crate::default_config_path()
        .parent()
        .map(|dir| dir.join("service-error.log"))
        .unwrap_or_else(|| std::path::PathBuf::from("service-error.log"));

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "[{now}] m5stack-pc-bridge service error: {err}");
    }
}

fn run_service() -> anyhow::Result<()> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

    let result = run_and_report_status(&status_handle, stop_rx);

    // 成功・失敗どちらの経路でも、SCMへ必ずStoppedを報告する。ここを怠ると、
    // 起動処理中のエラーなどでSCMへの応答が途絶え、「応答なし」という原因の
    // 分かりにくい汎用エラーになる。exit_codeは成功時のみ0とし、起動失敗や
    // server::serve_with_shutdownのErrはinstall.ps1のfailure action(自動再起動)を
    // 発動させるため非0で報告する。STOP/SHUTDOWN要求による通常停止はOk(())になるため
    // ここでは0のまま。
    let exit_code = if result.is_ok() {
        ServiceExitCode::Win32(0)
    } else {
        ServiceExitCode::Win32(1)
    };
    let _ = set_status(&status_handle, ServiceState::Stopped, false, exit_code);
    result
}

fn run_and_report_status(
    status_handle: &service_control_handler::ServiceStatusHandle,
    stop_rx: mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    set_status(
        status_handle,
        ServiceState::StartPending,
        false,
        ServiceExitCode::Win32(0),
    )?;

    let config_path = crate::default_config_path();
    let config = AgentConfig::from_path(&config_path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {e}", config_path.display()))?;

    let runtime = tokio::runtime::Runtime::new()?;
    let (graceful_tx, graceful_rx) = tokio::sync::oneshot::channel::<()>();

    // mpsc(同期)側のSTOP通知をtokio側のgraceful shutdown signalへ橋渡しする。
    let status_handle_for_stop = status_handle.clone();
    std::thread::spawn(move || {
        let _ = stop_rx.recv();
        // graceful shutdown中もRunningのまま報告し続けると、SCMが規定時間内に
        // 応答がないと判断することがあるため、停止処理に入ったことを即座に伝える。
        let _ = set_status(
            &status_handle_for_stop,
            ServiceState::StopPending,
            false,
            ServiceExitCode::Win32(0),
        );
        let _ = graceful_tx.send(());
    });

    set_status(
        status_handle,
        ServiceState::Running,
        true,
        ServiceExitCode::Win32(0),
    )?;

    runtime.block_on(server::serve_with_shutdown(config, async {
        let _ = graceful_rx.await;
    }))
}

fn set_status(
    status_handle: &service_control_handler::ServiceStatusHandle,
    state: ServiceState,
    accept_stop: bool,
    exit_code: ServiceExitCode,
) -> anyhow::Result<()> {
    let controls_accepted = if accept_stop {
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
    } else {
        ServiceControlAccept::empty()
    };

    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: state,
        controls_accepted,
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::from_secs(5),
        process_id: None,
    })?;
    Ok(())
}
