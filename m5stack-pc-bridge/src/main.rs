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
    use m5stack_pc_bridge::{server, Cli};

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Cli::parse();
    let config = m5stack_pc_bridge::load_config(args.config_path())?;
    server::serve(config).await
}
