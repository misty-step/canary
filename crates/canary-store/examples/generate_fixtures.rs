//! Generate Rust-owned SQLite fixture databases.

use std::{env, error::Error, path::PathBuf};

use canary_store::fixtures::{write_read_model_fixture, write_schema_fixture};

fn main() -> Result<(), Box<dyn Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        eprintln!(
            "usage: cargo run -p canary-store --example generate_fixtures -- <schema.db> <read_models.db>"
        );
        std::process::exit(64);
    }

    let schema_path = PathBuf::from(&args[0]);
    let read_model_path = PathBuf::from(&args[1]);

    write_schema_fixture(&schema_path)?;
    write_read_model_fixture(&read_model_path)?;

    println!("Regenerated {}", schema_path.display());
    println!("Regenerated {}", read_model_path.display());
    Ok(())
}
