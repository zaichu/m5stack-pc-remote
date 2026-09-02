use clap::Parser;
use pc_remote_agent::{app_config::AgentConfig, server};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, env = "PC_REMOTE_AGENT_CONFIG", default_value = "config.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let config = AgentConfig::from_path(args.config)?;
    server::serve(config).await
}
