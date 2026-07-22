use clap::Parser;
use tonic_h3_bench::cli::ClientArgs;
use tonic_h3_bench::{BenchError, client::run_client, init_tracing};

#[tokio::main]
async fn main() -> Result<(), BenchError> {
    let args = ClientArgs::parse();
    init_tracing();

    tracing::info!(
        "starting bench-client: transport={} addr={} payload={}B concurrency={}",
        args.transport,
        args.addr,
        args.payload_size,
        args.concurrency
    );

    let summary = run_client(&args).await?;
    println!("{summary}");

    if summary.success == 0 {
        return Err("no successful requests".into());
    }
    Ok(())
}
