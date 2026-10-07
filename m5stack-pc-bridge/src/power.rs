use std::process::Command;

use serde::Serialize;

use crate::suspend;

/// 電源操作の識別子はwire protocolの一部なので `pc-remote-signing` が正本。
/// bridge側は再エクスポートして使う。
pub use pc_remote_signing::PowerAction;

#[derive(Debug, Serialize)]
pub struct PowerResult {
    pub action: &'static str,
    pub dry_run: bool,
    pub command: Vec<String>,
    /// 応答の意味づけ。reboot/shutdownの200は実行済み(`"ok"`)だが、
    /// sleepの200は受理・実行開始(`"accepted"`)で実行成功ではない。
    /// sleepの実行結果は応答に含められず、監査ログとサーバログにだけ残る。
    pub result: &'static str,
}

pub fn run_power_action(action: PowerAction, dry_run: bool) -> anyhow::Result<PowerResult> {
    run_power_action_with_executor(action, dry_run, execute_shutdown)
}

pub fn run_power_action_with_executor<F>(
    action: PowerAction,
    dry_run: bool,
    executor: F,
) -> anyhow::Result<PowerResult>
where
    F: FnMut(&[&str]) -> anyhow::Result<()>,
{
    run_power_action_with_executors(action, dry_run, executor, suspend::suspend)
}

/// `Sleep` は `shutdown.exe` ではなく `SetSuspendState` のAPI呼び出しに
/// 振り分ける。`shutdown_executor` はReboot/Shutdown用、`suspend_executor`
/// はSleep用で、どちらも `dry_run` では呼ばない。テストでは本物の
/// `SetSuspendState` を呼ばず、注入したexecutorで成功・失敗を再現すること。
pub fn run_power_action_with_executors<S, P>(
    action: PowerAction,
    dry_run: bool,
    mut shutdown_executor: S,
    mut suspend_executor: P,
) -> anyhow::Result<PowerResult>
where
    S: FnMut(&[&str]) -> anyhow::Result<()>,
    P: FnMut() -> anyhow::Result<()>,
{
    match action {
        PowerAction::Reboot | PowerAction::Shutdown => {
            let flag = if action == PowerAction::Reboot {
                "/r"
            } else {
                "/s"
            };
            let args = vec![flag, "/t", "0"];
            let command = std::iter::once("shutdown.exe".to_string())
                .chain(args.iter().map(|s| (*s).to_string()))
                .collect::<Vec<_>>();

            if !dry_run {
                shutdown_executor(&args)?;
            }

            Ok(PowerResult {
                action: action.slug(),
                dry_run,
                command,
                result: "ok",
            })
        }
        PowerAction::Sleep => {
            if !dry_run {
                suspend_executor()?;
            }

            Ok(accepted_sleep_result(dry_run))
        }
    }
}

/// sleep受理応答の本文。`SetSuspendState` の引数をそのまま残す
/// (何を呼ぶ/呼んだかの監査用)。実行はせず、呼び出し側(`server.rs`)が
/// 別スレッドで遅延実行する。`dry_run` ではexecutorを呼ばない。
pub fn accepted_sleep_result(dry_run: bool) -> PowerResult {
    // `SetSuspendState(hibernate=false, force=false, disable_wake_event=false)`
    // の引数をそのまま残す(何を呼ぶかの監査用)。
    let command = ["SetSuspendState", "false", "false", "false"]
        .into_iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    PowerResult {
        action: PowerAction::Sleep.slug(),
        dry_run,
        command,
        result: "accepted",
    }
}

fn execute_shutdown(args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new("shutdown.exe").args(args).status()?;
    if !status.success() {
        anyhow::bail!("shutdown.exe exited with status {status}");
    }
    Ok(())
}
