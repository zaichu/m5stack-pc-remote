use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Copy)]
pub enum PowerAction {
    Reboot,
    Shutdown,
}

#[derive(Debug, Serialize)]
pub struct PowerResult {
    pub action: &'static str,
    pub dry_run: bool,
    pub command: Vec<String>,
}

pub fn run_power_action(action: PowerAction, dry_run: bool) -> anyhow::Result<PowerResult> {
    run_power_action_with_executor(action, dry_run, execute_shutdown)
}

pub fn run_power_action_with_executor<F>(
    action: PowerAction,
    dry_run: bool,
    mut executor: F,
) -> anyhow::Result<PowerResult>
where
    F: FnMut(&[&str]) -> anyhow::Result<()>,
{
    let args = match action {
        PowerAction::Reboot => vec!["/r", "/t", "0"],
        PowerAction::Shutdown => vec!["/s", "/t", "0"],
    };
    let command = std::iter::once("shutdown.exe".to_string())
        .chain(args.iter().map(|s| (*s).to_string()))
        .collect::<Vec<_>>();

    if !dry_run {
        executor(&args)?;
    }

    Ok(PowerResult {
        action: match action {
            PowerAction::Reboot => "reboot",
            PowerAction::Shutdown => "shutdown",
        },
        dry_run,
        command,
    })
}

fn execute_shutdown(args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new("shutdown.exe").args(args).status()?;
    if !status.success() {
        anyhow::bail!("shutdown.exe exited with status {status}");
    }
    Ok(())
}
