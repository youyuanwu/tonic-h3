use clap::Parser;
use tonic_h3_bench::cli::ServerArgs;
use tonic_h3_bench::{BenchError, init_tracing, server::run_server};

#[tokio::main]
async fn main() -> Result<(), BenchError> {
    let args = ServerArgs::parse();
    init_tracing();

    let addr = args
        .addr
        .parse()
        .map_err(|e| format!("invalid --addr `{}`: {e}", args.addr))?;

    tracing::info!(
        "starting bench-server: transport={} addr={}",
        args.transport,
        addr
    );

    // Serve until Ctrl-C.
    let shutdown = async {
        wait_for_shutdown_signal().await;
        tracing::info!("shutdown signal received");
    };

    run_server(args.transport, addr, shutdown).await?;
    tracing::info!("bench-server stopped");
    Ok(())
}

/// Resolve on the first of SIGINT (Ctrl-C) or, on Unix, SIGTERM.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
