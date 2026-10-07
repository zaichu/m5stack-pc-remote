use anyhow::anyhow;
use m5stack_pc_bridge::power::{
    accepted_sleep_result, run_power_action_with_executor, run_power_action_with_executors,
    PowerAction,
};

#[test]
fn dry_run_does_not_execute_shutdown_command() {
    let mut called = false;

    let result = run_power_action_with_executor(PowerAction::Shutdown, true, |_| {
        called = true;
        Ok(())
    })
    .unwrap();

    assert!(!called);
    assert!(result.dry_run);
    assert_eq!(result.command[0], "shutdown.exe");
    // reboot/shutdownの200は実行済みの意味。
    assert_eq!(result.result, "ok");
}

#[test]
fn non_dry_run_reports_executor_failure() {
    let err = run_power_action_with_executor(PowerAction::Reboot, false, |_| {
        Err(anyhow!("simulated shutdown.exe failure"))
    })
    .unwrap_err();

    assert!(err.to_string().contains("simulated shutdown.exe failure"));
}

#[test]
fn sleep_dry_run_does_not_call_suspend_executor() {
    let mut shutdown_called = false;
    let mut suspend_called = false;

    let result = run_power_action_with_executors(
        PowerAction::Sleep,
        true,
        |_: &[&str]| {
            shutdown_called = true;
            Ok(())
        },
        || {
            suspend_called = true;
            Ok(())
        },
    )
    .unwrap();

    assert!(!shutdown_called);
    assert!(!suspend_called);
    assert!(result.dry_run);
    assert_eq!(result.action, "sleep");
    assert_eq!(result.command[0], "SetSuspendState");
    // sleepの200は受理の意味で、実行成功ではない。
    assert_eq!(result.result, "accepted");
}

#[test]
fn sleep_uses_suspend_executor_instead_of_shutdown() {
    let mut shutdown_called = false;
    let mut suspend_called = false;

    let result = run_power_action_with_executors(
        PowerAction::Sleep,
        false,
        |_: &[&str]| {
            shutdown_called = true;
            Ok(())
        },
        || {
            suspend_called = true;
            Ok(())
        },
    )
    .unwrap();

    assert!(!shutdown_called);
    assert!(suspend_called);
    assert!(!result.dry_run);
    assert_eq!(result.action, "sleep");
    assert_eq!(result.result, "accepted");
}

#[test]
fn sleep_reports_suspend_executor_failure() {
    let err = run_power_action_with_executors(
        PowerAction::Sleep,
        false,
        |_: &[&str]| Ok(()),
        || Err(anyhow!("simulated SetSuspendState failure")),
    )
    .unwrap_err();

    assert!(err
        .to_string()
        .contains("simulated SetSuspendState failure"));
}

#[test]
fn accepted_sleep_result_builds_response_without_executing() {
    // 受理応答の本文組み立てはexecutorを呼ばない。実行はserver側の
    // 別スレッドが行うため、ここでは本文の形だけを固定する。
    for dry_run in [true, false] {
        let result = accepted_sleep_result(dry_run);
        assert_eq!(result.action, "sleep");
        assert_eq!(result.dry_run, dry_run);
        assert_eq!(result.command[0], "SetSuspendState");
        assert_eq!(result.result, "accepted");
    }
}
