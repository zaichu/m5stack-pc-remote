use anyhow::anyhow;
use m5stack_pc_bridge::power::{run_power_action_with_executor, PowerAction};

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
}

#[test]
fn non_dry_run_reports_executor_failure() {
    let err = run_power_action_with_executor(PowerAction::Reboot, false, |_| {
        Err(anyhow!("simulated shutdown.exe failure"))
    })
    .unwrap_err();

    assert!(err.to_string().contains("simulated shutdown.exe failure"));
}
