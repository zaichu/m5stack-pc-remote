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
        eprintln!("m5stack-pc-bridge service error: {e}");
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
    set_status(&status_handle, ServiceState::StartPending, false)?;

    let config_path = crate::default_config_path();
    let config = AgentConfig::from_path(&config_path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {e}", config_path.display()))?;

    let runtime = tokio::runtime::Runtime::new()?;
    let (graceful_tx, graceful_rx) = tokio::sync::oneshot::channel::<()>();

    // mpsc(同期)側のSTOP通知をtokio側のgraceful shutdown signalへ橋渡しする。
    std::thread::spawn(move || {
        let _ = stop_rx.recv();
        let _ = graceful_tx.send(());
    });

    set_status(&status_handle, ServiceState::Running, true)?;

    let result = runtime.block_on(server::serve_with_shutdown(config, async {
        let _ = graceful_rx.await;
    }));

    set_status(&status_handle, ServiceState::Stopped, false)?;
    result
}

fn set_status(
    status_handle: &service_control_handler::ServiceStatusHandle,
    state: ServiceState,
    accept_stop: bool,
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
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::from_secs(5),
        process_id: None,
    })?;
    Ok(())
}
