//! Production process entrypoint for the Rust Canary server.

use std::{error::Error, path::PathBuf, process};

use canary_http::public::CANARY_VERSION;
use canary_server::{CanaryServer, ServerProcessConfig, keygen};
use canary_store::{CURRENT_SCHEMA_VERSION, Store, vacuum_database, verify_database};
use serde_json::json;
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = run_operator_command(&args) {
        return result;
    }

    let config = ServerProcessConfig::from_env(std::env::vars())?;
    let listener = tokio::net::TcpListener::bind(config.listen_addr).await?;
    let local_addr = listener.local_addr()?;
    let server = CanaryServer::boot(config.server)?;

    info!(%local_addr, "canary-server listening");
    server.serve(listener, shutdown_signal()).await?;
    Ok(())
}

fn run_operator_command(args: &[String]) -> Option<Result<(), Box<dyn Error>>> {
    match args.first().map(String::as_str) {
        Some("version" | "--version" | "-V") => Some(run_version(&args[1..])),
        Some("verify-data") => Some(run_verify_data(&args[1..], false)),
        Some("migrate") => Some(run_verify_data(&args[1..], true)),
        Some("vacuum-database") => Some(run_vacuum_database(&args[1..])),
        // Recovery path for issuing a scoped API key directly against the
        // SQLite store when the one-time bootstrap key has been lost.
        Some("mint-key") => Some(run_mint_key(&args[1..])),
        Some(_) | None => None,
    }
}

fn run_version(args: &[String]) -> Result<(), Box<dyn Error>> {
    if !args.is_empty() {
        return Err("version takes no arguments".into());
    }
    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema": "canary.runtime-version.v1",
            "version": CANARY_VERSION,
            "database_schema_version": CURRENT_SCHEMA_VERSION,
        }))?
    );
    Ok(())
}

fn run_verify_data(args: &[String], migrate: bool) -> Result<(), Box<dyn Error>> {
    let database_path = parse_database_path(args)?;
    if migrate {
        let mut store = Store::open(&database_path)?;
        store.migrate()?;
    }
    let evidence = verify_database(&database_path)?;
    println!("{}", serde_json::to_string(&evidence)?);
    if !evidence.verified() {
        return Err("database verification failed".into());
    }
    Ok(())
}

fn run_vacuum_database(args: &[String]) -> Result<(), Box<dyn Error>> {
    let database_path = parse_database_path(args)?;
    let report = vacuum_database(&database_path)?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema": "canary.vacuum_database.v1",
            "database": database_path,
            "page_size": report.page_size,
            "page_count_before": report.page_count_before,
            "page_count_after": report.page_count_after,
            "freelist_pages_before": report.freelist_pages_before,
            "freelist_pages_after": report.freelist_pages_after,
            "bytes_before": report.page_count_before.saturating_mul(report.page_size),
            "bytes_after": report.page_count_after.saturating_mul(report.page_size),
        }))?
    );
    Ok(())
}

fn parse_database_path(args: &[String]) -> Result<PathBuf, Box<dyn Error>> {
    let mut database_path = std::env::var("CANARY_DB_PATH").ok().map(PathBuf::from);
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--database" => {
                let value = iter.next().ok_or("--database requires a value")?;
                database_path = Some(PathBuf::from(value));
            }
            other => return Err(format!("unknown database command argument: {other}").into()),
        }
    }
    database_path.ok_or_else(|| {
        "database path required: pass --database or set CANARY_DB_PATH"
            .to_owned()
            .into()
    })
}

fn run_mint_key(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut scope = "admin".to_owned();
    let mut name = "operator-minted".to_owned();
    let mut service: Option<String> = None;
    let mut allow_unbound = false;
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
            "--allow-unbound" => allow_unbound = true,
            other => return Err(format!("unknown mint-key argument: {other}").into()),
        }
    }

    let db_path = std::env::var("CANARY_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DB_PATH));

    let raw_key = keygen::mint_key(&db_path, &scope, &name, service.as_deref(), allow_unbound)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_commands_require_an_explicit_path() {
        // Unit-test the parser with a known empty argument list only when the
        // process environment is not supplying the product input.
        if std::env::var_os("CANARY_DB_PATH").is_none() {
            assert!(parse_database_path(&[]).is_err());
        }
    }

    #[test]
    fn database_commands_parse_only_the_portable_database_input() -> Result<(), Box<dyn Error>> {
        assert_eq!(
            parse_database_path(&["--database".to_owned(), "/tmp/restored.db".to_owned()])?,
            PathBuf::from("/tmp/restored.db")
        );
        assert!(parse_database_path(&["--host".to_owned()]).is_err());
        Ok(())
    }
}
