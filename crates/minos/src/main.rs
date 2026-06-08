//! Minos entrypoint: initialize logging, read config from the environment,
//! and run until the web server stops.

use minos::{run, AppConfig};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Honor RUST_LOG; default to `info` when unset or unparseable.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    run(AppConfig::from_env()).await
}
