//! Production process entrypoint for the Rust Canary server.

use std::{error::Error, path::PathBuf, process};

use canary_server::{CanaryServer, ServerProcessConfig, keygen};
use tracing::info;
use tracing_subscriber::EnvFilter;

const DEFAULT_DB_PATH: &str = "/data/canary.db";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    if let Err(error) = run().await {
        eprintln!("canary-server: {error}");
        process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    // Operator subcommand: `canary-server mint-key [--scope S] [--name N] [--service SERVICE]`.
    // Recovery path for issuing a scoped API key directly against the SQLite
    // store when the one-time bootstrap key has been lost. Defaults to admin.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("mint-key") {
        return run_mint_key(&args[1..]);
    }

    let config = ServerProcessConfig::from_env(std::env::vars())?;
    let listener = tokio::net::TcpListener::bind(config.listen_addr).await?;
    let local_addr = listener.local_addr()?;
    let server = CanaryServer::boot(config.server)?;

    info!(%local_addr, "canary-server listening");
    server.serve(listener, shutdown_signal()).await?;
    Ok(())
}

fn run_mint_key(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut scope = "admin".to_owned();
    let mut name = "operator-minted".to_owned();
    let mut service: Option<String> = None;
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--scope" => {
                scope = iter.next().ok_or("--scope requires a value")?.clone();
            }
            "--name" => {
                name = iter.next().ok_or("--name requires a value")?.clone();
            }
            "--service" => {
                service = Some(iter.next().ok_or("--service requires a value")?.clone());
            }
            other => return Err(format!("unknown mint-key argument: {other}").into()),
        }
    }

    let db_path = std::env::var("CANARY_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DB_PATH));

    let raw_key = keygen::mint_key(&db_path, &scope, &name, service.as_deref())?;
    // The raw key prints to stdout; everything else goes to stderr so the key
    // can be captured cleanly (e.g. `... mint-key | tail -1`).
    eprintln!(
        "Minted {scope} API key {name:?} against {}",
        db_path.display()
    );
    eprintln!("Store this key securely - it will not be shown again.");
    println!("{raw_key}");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "failed to install Ctrl-C handler");
        }
    };

    #[cfg(unix)]
    {
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to install SIGTERM handler");
                }
            }
        };

        tokio::select! {
            () = ctrl_c => {}
            () = terminate => {}
        }
    }

    #[cfg(not(unix))]
    ctrl_c.await;
}
