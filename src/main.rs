use std::io;

use tines_mcp::{config::Config, server};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    let listener = TcpListener::bind(config.bind_addr).await?;
    let address = listener.local_addr()?;
    tracing::info!(%address, "Tines MCP server listening");

    let cancellation_token = CancellationToken::new();
    let server_token = cancellation_token.clone();
    let mut server_task =
        tokio::spawn(async move { server::run(listener, &config, server_token).await });

    tokio::select! {
        result = &mut server_task => result??,
        result = shutdown_signal() => {
            result?;
            cancellation_token.cancel();
            (&mut server_task).await??;
        }
    }
    Ok(())
}

async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}
