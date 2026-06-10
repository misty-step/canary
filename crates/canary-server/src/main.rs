//! Production process entrypoint for the Rust Canary server.

use std::{error::Error, process};

use canary_server::{CanaryServer, ServerProcessConfig};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("canary-server: {error}");
        process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let config = ServerProcessConfig::from_env(std::env::vars())?;
    let listener = tokio::net::TcpListener::bind(config.listen_addr).await?;
    let local_addr = listener.local_addr()?;
    let server = CanaryServer::boot(config.server)?;

    eprintln!("canary-server listening on {local_addr}");
    server.serve(listener, shutdown_signal()).await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("canary-server: failed to install Ctrl-C handler: {error}");
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
                    eprintln!("canary-server: failed to install SIGTERM handler: {error}");
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
