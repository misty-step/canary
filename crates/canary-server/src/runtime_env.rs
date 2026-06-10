//! Process environment adapter for the Rust Canary server.
//!
//! Phoenix still defines the production environment names. This module keeps
//! that compatibility at the process edge and returns a typed runtime config so
//! the server boot path does not parse environment variables itself.

use std::{collections::BTreeMap, fmt, net::SocketAddr, path::PathBuf};

use canary_workers::retention::RetentionPolicy;

use crate::{ServerConfig, TargetProbeOptions};

const DEFAULT_DATABASE_PATH: &str = "/data/canary.db";
const DEFAULT_PORT: u16 = 4000;
const DEFAULT_ERROR_RETENTION_DAYS: i64 = 30;
const DEFAULT_CHECK_RETENTION_DAYS: i64 = 7;
const DISCLOSE_BOOTSTRAP_KEY_ENV: &str = "CANARY_DISCLOSE_BOOTSTRAP_KEY";

/// Fully parsed process configuration for the Rust server binary.
#[derive(Debug, Clone)]
pub struct ServerProcessConfig {
    /// Runtime configuration passed to `CanaryServer::boot`.
    pub server: ServerConfig,
    /// Socket address the process should bind.
    pub listen_addr: SocketAddr,
}

impl ServerProcessConfig {
    /// Build process configuration from key/value environment pairs.
    pub fn from_env<I, K, V>(vars: I) -> Result<Self, RuntimeEnvError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let vars = vars
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect::<BTreeMap<_, _>>();

        let database_path = env_value(&vars, "CANARY_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATABASE_PATH));
        let port = parse_port(env_value(&vars, "PORT"))?;
        let allow_private_targets = env_value(&vars, "ALLOW_PRIVATE_TARGETS") == Some("true");
        let error_retention_days = parse_i64(
            "ERROR_RETENTION_DAYS",
            env_value(&vars, "ERROR_RETENTION_DAYS"),
            DEFAULT_ERROR_RETENTION_DAYS,
        )?;
        let check_retention_days = parse_i64(
            "CHECK_RETENTION_DAYS",
            env_value(&vars, "CHECK_RETENTION_DAYS"),
            DEFAULT_CHECK_RETENTION_DAYS,
        )?;
        let disclose_bootstrap_key = parse_bool(
            DISCLOSE_BOOTSTRAP_KEY_ENV,
            env_value(&vars, DISCLOSE_BOOTSTRAP_KEY_ENV),
        )?
        .unwrap_or(true);

        let mut server = ServerConfig::new(database_path);
        server.target_probe_options = TargetProbeOptions {
            allow_private_targets,
            ..TargetProbeOptions::default()
        };
        server.retention_policy = RetentionPolicy {
            error_retention_days,
            check_retention_days,
        };
        server.disclose_bootstrap_key = disclose_bootstrap_key;

        Ok(Self {
            server,
            listen_addr: SocketAddr::from(([0, 0, 0, 0], port)),
        })
    }
}

/// Invalid process environment for the Rust server binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEnvError {
    variable: &'static str,
    detail: String,
}

impl RuntimeEnvError {
    fn new(variable: &'static str, detail: impl Into<String>) -> Self {
        Self {
            variable,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for RuntimeEnvError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} environment variable: {}",
            self.variable, self.detail
        )
    }
}

impl std::error::Error for RuntimeEnvError {}

fn env_value<'a>(vars: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    vars.get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

fn parse_port(value: Option<&str>) -> Result<u16, RuntimeEnvError> {
    match value {
        Some(value) => value.parse::<u16>().map_err(|error| {
            RuntimeEnvError::new("PORT", format!("expected TCP port 0-65535 ({error})"))
        }),
        None => Ok(DEFAULT_PORT),
    }
}

fn parse_i64(
    variable: &'static str,
    value: Option<&str>,
    default: i64,
) -> Result<i64, RuntimeEnvError> {
    match value {
        Some(value) => value
            .parse::<i64>()
            .map_err(|error| RuntimeEnvError::new(variable, format!("expected integer ({error})"))),
        None => Ok(default),
    }
}

fn parse_bool(
    variable: &'static str,
    value: Option<&str>,
) -> Result<Option<bool>, RuntimeEnvError> {
    match value {
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(value) => Err(RuntimeEnvError::new(
            variable,
            format!("expected true or false, got {value:?}"),
        )),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_config_uses_phoenix_compatible_defaults() -> Result<(), RuntimeEnvError> {
        let config = ServerProcessConfig::from_env(Vec::<(String, String)>::new())?;

        assert_eq!(
            config.server.database_path,
            PathBuf::from("/data/canary.db")
        );
        assert_eq!(config.listen_addr, SocketAddr::from(([0, 0, 0, 0], 4000)));
        assert_eq!(config.server.retention_policy, RetentionPolicy::default());
        assert!(!config.server.target_probe_options.allow_private_targets);
        assert!(config.server.disclose_bootstrap_key);
        Ok(())
    }

    #[test]
    fn process_config_reads_production_environment() -> Result<(), RuntimeEnvError> {
        let config = ServerProcessConfig::from_env([
            ("CANARY_DB_PATH", "/tmp/canary.db"),
            ("PORT", "8080"),
            ("ALLOW_PRIVATE_TARGETS", "true"),
            ("ERROR_RETENTION_DAYS", "90"),
            ("CHECK_RETENTION_DAYS", "14"),
            ("CANARY_DISCLOSE_BOOTSTRAP_KEY", "false"),
        ])?;

        assert_eq!(config.server.database_path, PathBuf::from("/tmp/canary.db"));
        assert_eq!(config.listen_addr, SocketAddr::from(([0, 0, 0, 0], 8080)));
        assert_eq!(
            config.server.retention_policy,
            RetentionPolicy {
                error_retention_days: 90,
                check_retention_days: 14,
            }
        );
        assert!(config.server.target_probe_options.allow_private_targets);
        assert!(!config.server.disclose_bootstrap_key);
        Ok(())
    }

    #[test]
    fn process_config_rejects_invalid_numeric_environment() {
        assert_eq!(
            error_variable(ServerProcessConfig::from_env([("PORT", "70000")])),
            Some("PORT")
        );
        assert_eq!(
            error_variable(ServerProcessConfig::from_env([(
                "ERROR_RETENTION_DAYS",
                "thirty"
            )])),
            Some("ERROR_RETENTION_DAYS")
        );
        assert_eq!(
            error_variable(ServerProcessConfig::from_env([(
                "CANARY_DISCLOSE_BOOTSTRAP_KEY",
                "0"
            )])),
            Some("CANARY_DISCLOSE_BOOTSTRAP_KEY")
        );
    }

    fn error_variable(
        result: Result<ServerProcessConfig, RuntimeEnvError>,
    ) -> Option<&'static str> {
        result.err().map(|error| error.variable)
    }
}
