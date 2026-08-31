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
    let args = match action {
        PowerAction::Reboot => vec!["/r", "/t", "0"],
        PowerAction::Shutdown => vec!["/s", "/t", "0"],
    };
    let command = std::iter::once("shutdown.exe".to_string())
        .chain(args.iter().map(|s| (*s).to_string()))
        .collect::<Vec<_>>();

    if !dry_run {
        Command::new("shutdown.exe").args(args).spawn()?;
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
