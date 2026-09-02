#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    m5stack_pc_bridge::windows_service::run()
}

/// Windows以外(Linux/WSL上の開発・`cargo test`が動くホスト)向け。実機のWindows Service
/// 経路は`windows_service`モジュールが担う。
#[cfg(not(windows))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use clap::Parser;
    use m5stack_pc_bridge::{app_config::AgentConfig, server};

    #[derive(Debug, Parser)]
    struct Args {
        #[arg(long, env = "M5STACK_PC_BRIDGE_CONFIG")]
        config: Option<String>,
    }

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let config_path = args
        .config
        .map(std::path::PathBuf::from)
        .unwrap_or_else(m5stack_pc_bridge::default_config_path);
    let config = AgentConfig::from_path(&config_path)?;
    server::serve(config).await
}
