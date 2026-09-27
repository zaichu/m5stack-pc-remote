//! Windows Service(SCM)としての起動。`cfg(windows)`専用。
//! `install.ps1` の `New-Service` 登録名と `SERVICE_NAME` は一致している必要がある。

use std::ffi::OsString;
use std::sync::mpsc;
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use crate::server;

pub const SERVICE_NAME: &str = "M5StackPcBridge";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

/// SCM経由でないプロセスから `StartServiceCtrlDispatcher` を呼んだときのWin32エラー。
/// Windows API固定値のため直接依存を増やさずハードコードする。
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
    // foreground 実行では `--config` / `M5STACK_PC_BRIDGE_CONFIG` を解釈する。
    // `std::env::args()` はSCMが `service_main` へ渡す引数とは別物なので混同しないこと。
    let cli_config = crate::parse_config_arg(std::env::args_os().skip(1));
    let env_config = crate::env_config_path(std::env::var_os(crate::CONFIG_ENV_VAR));
    let config = crate::load_foreground_config(cli_config, env_config)?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(server::serve(config))
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    // Windows側の規約でResultを返せない。失敗は異常終了としてSCMへ伝わり、
    // install.ps1のrecovery(自動再起動)に任せる。
    // `_arguments` はservice登録引数でforeground時の `--config` とは別物なので無視する。
    // 設定パスは常に実行ファイル横のconfig.toml。
    if let Err(e) = run_service() {
        // Windows Serviceにはコンソールが無いため、実行ファイル横のログファイルへ
        // 書いて起動失敗の原因を追えるようにする(secretは書かない)。
        log_startup_error(&e);
    }
}

fn log_startup_error(err: &anyhow::Error) {
    use std::io::Write;

    let log_path = crate::exe_dir_file("service-error.log");

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

    // 成功・失敗どちらでもSCMへ必ずStoppedを報告する。怠ると応答が途絶えて
    // 「応答なし」の分かりにくい汎用エラーになる。exit_codeは失敗時のみ非0とし、
    // install.ps1のfailure action(自動再起動)を発動させる。
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

    // service起動時の設定は実行ファイル横に固定する(`--config`/環境変数の解決はしない)。
    let config = crate::load_default_config()?;

    let runtime = tokio::runtime::Runtime::new()?;
    let (graceful_tx, graceful_rx) = tokio::sync::oneshot::channel::<()>();

    // mpsc(同期)側のSTOP通知をtokio側のgraceful shutdown signalへ橋渡しする。
    let status_handle_for_stop = status_handle.clone();
    std::thread::spawn(move || {
        let _ = stop_rx.recv();
        // RunningのままだとSCMが「応答なし」と判断し得るため、停止処理に入ったことを即伝える。
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
