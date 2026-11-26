use anyhow::{Context, Result, ensure};
use clap::{Parser, ValueEnum};
use std::{future::IntoFuture, net::IpAddr, sync::Arc, time::Duration};
use suunto_mcp::{App, config::Config, mcp};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, ValueEnum)]
enum Transport {
    Stdio,
    Http,
    Sse,
    StreamableHttp,
}

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// MCP transport.
    #[arg(long, default_value = "stdio")]
    transport: Transport,
    /// HTTP/SSE bind host.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// HTTP/SSE bind port.
    #[arg(long, default_value_t = 8000)]
    port: u16,
    /// Probe the service on loopback. Used by the container health check.
    #[arg(long, hide = true)]
    healthcheck: bool,
}

// Fixed worker count keeps thread stacks modest under the container memory limit.
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.healthcheck {
        return probe(cli.port).await;
    }
    let config = Config::load()?;
    if matches!(cli.transport, Transport::Stdio) {
        return mcp::stdio(Arc::new(App::new(config)?)).await;
    }

    let host = cli
        .host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(&cli.host);
    let localhost = host.eq_ignore_ascii_case("localhost");
    let loopback = localhost || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    ensure!(
        loopback || !config.bearer.is_empty(),
        "SUUNTO_MCP_API_TOKEN is required for non-local HTTP/SSE binds"
    );
    // Do not resolve localhost through the system resolver for unauthenticated binds.
    let host = if localhost { "127.0.0.1" } else { host };
    let listener = TcpListener::bind((host, cli.port))
        .await
        .context("Bind HTTP listener")?;
    let app = Arc::new(App::new(config)?);
    eprintln!("Suunto MCP listening on {}", listener.local_addr()?);
    let shutdown = CancellationToken::new();
    let server = axum::serve(
        listener,
        mcp::router(app, matches!(cli.transport, Transport::Sse)),
    )
    .with_graceful_shutdown(shutdown.clone().cancelled_owned())
    .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result?,
        () = shutdown_signal() => {
            shutdown.cancel();
            // A legacy event stream can otherwise keep graceful shutdown open until its idle TTL.
            if let Ok(result) = tokio::time::timeout(Duration::from_secs(5), server).await {
                result?;
            }
        }
    }
    Ok(())
}

async fn probe(port: u16) -> Result<()> {
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(2))
        .build()?
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "Health probe returned {}",
        response.status()
    );
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => (),
                _ = terminate.recv() => (),
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
