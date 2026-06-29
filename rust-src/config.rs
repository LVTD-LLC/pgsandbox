use std::{env, fs};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::local::{LocalPostgresCluster, LOCAL_PROFILE_NAME};

const DEFAULT_DATABASE_PREFIX: &str = "pgsandbox";
const DEFAULT_TTL_MINUTES: u32 = 240;
const DEFAULT_MAX_TTL_MINUTES: u32 = 1440;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("PGSANDBOX_CONFIG must contain at least one profile.")]
    MissingProfiles,
    #[error("failed to read config file {path}: {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config JSON: {0}")]
    ParseJson(#[from] serde_json::Error),
    #[error("profile name cannot be empty")]
    EmptyProfileName,
    #[error("profile adminUrl cannot be empty")]
    EmptyAdminUrl,
    #[error("defaultTtlMinutes cannot exceed maxTtlMinutes for profile {0}")]
    InvalidTtl(String),
    #[error("default profile does not exist: {0}")]
    MissingDefaultProfile(String),
    #[error("Unknown Postgres profile: {0}")]
    UnknownProfile(String),
    #[error("failed to prepare default local Postgres cluster: {0}")]
    LocalPostgres(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxConfig {
    pub default_profile: String,
    pub profiles: Vec<SandboxProfile>,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxProfile {
    pub name: String,
    pub admin_url: String,
    #[serde(default = "default_database_prefix")]
    pub database_prefix: String,
    #[serde(default = "default_ttl_minutes")]
    pub default_ttl_minutes: u32,
    #[serde(default = "default_max_ttl_minutes")]
    pub max_ttl_minutes: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryConfig {
    #[serde(default = "default_telemetry_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawConfig {
    default_profile: Option<String>,
    profiles: Vec<SandboxProfile>,
    #[serde(default)]
    telemetry: TelemetryConfig,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: default_telemetry_enabled(),
        }
    }
}

pub fn load_config() -> Result<SandboxConfig, ConfigError> {
    load_config_from_env(env::vars())
}

pub fn load_config_from_env<I, K, V>(vars: I) -> Result<SandboxConfig, ConfigError>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    load_config_from_env_with_local(vars, || {
        let config = LocalPostgresCluster::from_env()
            .map_err(|error| ConfigError::LocalPostgres(error.to_string()))?
            .ensure_started()
            .map_err(|error| ConfigError::LocalPostgres(error.to_string()))?;
        Ok(config.admin_url)
    })
}

fn load_config_from_env_with_local<I, K, V, F>(
    vars: I,
    local_admin_url: F,
) -> Result<SandboxConfig, ConfigError>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
    F: FnOnce() -> Result<String, ConfigError>,
{
    let env = vars
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect::<std::collections::HashMap<_, _>>();

    let mut config = if let Some(path) = env.get("PGSANDBOX_CONFIG") {
        parse_config_file(
            &fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
                path: path.clone(),
                source,
            })?,
        )?
    } else {
        let explicit_default_profile = env.get("PGSANDBOX_DEFAULT_PROFILE").cloned();
        let (admin_url, name) = match env.get("PGSANDBOX_ADMIN_DATABASE_URL") {
            Some(admin_url) => (
                admin_url.to_string(),
                explicit_default_profile.unwrap_or_else(|| "default".to_string()),
            ),
            None => {
                let default_profile =
                    explicit_default_profile.unwrap_or_else(|| LOCAL_PROFILE_NAME.to_string());
                if default_profile != LOCAL_PROFILE_NAME {
                    return Err(ConfigError::MissingDefaultProfile(default_profile));
                }
                (local_admin_url()?, LOCAL_PROFILE_NAME.to_string())
            }
        };

        normalize_config(RawConfig {
            default_profile: Some(name.clone()),
            profiles: vec![SandboxProfile {
                name,
                admin_url,
                database_prefix: env
                    .get("PGSANDBOX_DATABASE_PREFIX")
                    .cloned()
                    .unwrap_or_else(default_database_prefix),
                default_ttl_minutes: env
                    .get("PGSANDBOX_DEFAULT_TTL_MINUTES")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(DEFAULT_TTL_MINUTES),
                max_ttl_minutes: env
                    .get("PGSANDBOX_MAX_TTL_MINUTES")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(DEFAULT_MAX_TTL_MINUTES),
            }],
            telemetry: TelemetryConfig::default(),
        })?
    };

    apply_telemetry_env_overrides(&mut config.telemetry, &env);
    Ok(config)
}

pub fn parse_config_file(raw_json: &str) -> Result<SandboxConfig, ConfigError> {
    normalize_config(serde_json::from_str(raw_json)?)
}

pub fn find_profile<'a>(
    config: &'a SandboxConfig,
    profile_name: Option<&str>,
) -> Result<&'a SandboxProfile, ConfigError> {
    let name = profile_name.unwrap_or(&config.default_profile);
    config
        .profiles
        .iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| ConfigError::UnknownProfile(name.to_string()))
}

pub fn load_telemetry_config() -> TelemetryConfig {
    telemetry_config_from_env(env::vars())
}

fn normalize_config(raw: RawConfig) -> Result<SandboxConfig, ConfigError> {
    if raw.profiles.is_empty() {
        return Err(ConfigError::MissingProfiles);
    }

    for profile in &raw.profiles {
        if profile.name.trim().is_empty() {
            return Err(ConfigError::EmptyProfileName);
        }
        if profile.admin_url.trim().is_empty() {
            return Err(ConfigError::EmptyAdminUrl);
        }
        if profile.default_ttl_minutes > profile.max_ttl_minutes {
            return Err(ConfigError::InvalidTtl(profile.name.clone()));
        }
    }

    let default_profile = raw
        .default_profile
        .clone()
        .unwrap_or_else(|| raw.profiles[0].name.clone());

    if !raw
        .profiles
        .iter()
        .any(|profile| profile.name == default_profile)
    {
        return Err(ConfigError::MissingDefaultProfile(default_profile));
    }

    Ok(SandboxConfig {
        default_profile,
        profiles: raw.profiles,
        telemetry: raw.telemetry,
    })
}

pub fn telemetry_config_from_env<I, K, V>(vars: I) -> TelemetryConfig
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let env = vars
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect::<std::collections::HashMap<_, _>>();
    let mut config = env
        .get("PGSANDBOX_CONFIG")
        .map(|path| telemetry_config_from_file(path).unwrap_or(TelemetryConfig { enabled: false }))
        .unwrap_or_default();
    apply_telemetry_env_overrides(&mut config, &env);
    config
}

fn telemetry_config_from_file(path: &str) -> Result<TelemetryConfig, ConfigError> {
    let raw = fs::read_to_string(path).map_err(|source| ConfigError::ReadFile {
        path: path.to_string(),
        source,
    })?;
    let value = serde_json::from_str::<serde_json::Value>(&raw)?;
    let Some(telemetry) = value.get("telemetry") else {
        return Ok(TelemetryConfig::default());
    };
    Ok(serde_json::from_value(telemetry.clone())?)
}

fn apply_telemetry_env_overrides(
    config: &mut TelemetryConfig,
    env: &std::collections::HashMap<String, String>,
) {
    if let Some(false) = env
        .get("PGSANDBOX_TELEMETRY")
        .and_then(|value| parse_bool_flag(value))
    {
        config.enabled = false;
    }

    if [
        "PGSANDBOX_NO_TELEMETRY",
        "PGSANDBOX_DISABLE_TELEMETRY",
        "DO_NOT_TRACK",
    ]
    .iter()
    .any(|key| {
        env.get(*key)
            .and_then(|value| parse_bool_flag(value))
            .unwrap_or(false)
    }) {
        config.enabled = false;
    }
}

fn parse_bool_flag(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn default_database_prefix() -> String {
    DEFAULT_DATABASE_PREFIX.to_string()
}

fn default_ttl_minutes() -> u32 {
    DEFAULT_TTL_MINUTES
}

fn default_max_ttl_minutes() -> u32 {
    DEFAULT_MAX_TTL_MINUTES
}

fn default_telemetry_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_single_admin_url_from_env() {
        let config = load_config_from_env([(
            "PGSANDBOX_ADMIN_DATABASE_URL",
            "postgres://postgres:postgres@localhost/postgres",
        )])
        .unwrap();

        assert_eq!(config.default_profile, "default");
        assert_eq!(config.profiles[0].database_prefix, "pgsandbox");
        assert_eq!(config.profiles[0].default_ttl_minutes, 240);
        assert!(config.telemetry.enabled);
    }

    #[test]
    fn loads_managed_local_profile_when_no_admin_url_is_set() {
        let mut called = false;
        let config = load_config_from_env_with_local(std::iter::empty::<(&str, &str)>(), || {
            called = true;
            Ok(
                "postgres://pgsandbox_admin:secret@127.0.0.1:65432/postgres?sslmode=disable"
                    .to_string(),
            )
        })
        .unwrap();

        assert!(called);
        assert_eq!(config.default_profile, "local");
        assert_eq!(config.profiles[0].name, "local");
        assert_eq!(
            config.profiles[0].admin_url,
            "postgres://pgsandbox_admin:secret@127.0.0.1:65432/postgres?sslmode=disable"
        );
        assert_eq!(config.profiles[0].database_prefix, "pgsandbox");
        assert_eq!(config.profiles[0].default_ttl_minutes, 240);
        assert_eq!(config.profiles[0].max_ttl_minutes, 1440);
    }

    #[test]
    fn explicit_admin_url_skips_managed_local_profile() {
        let config = load_config_from_env_with_local(
            [(
                "PGSANDBOX_ADMIN_DATABASE_URL",
                "postgres://postgres:postgres@localhost/postgres",
            )],
            || panic!("local cluster should not start when an admin URL is explicit"),
        )
        .unwrap();

        assert_eq!(config.default_profile, "default");
        assert_eq!(
            config.profiles[0].admin_url,
            "postgres://postgres:postgres@localhost/postgres"
        );
    }

    #[test]
    fn default_profile_without_admin_url_does_not_alias_local_profile() {
        let err =
            load_config_from_env_with_local([("PGSANDBOX_DEFAULT_PROFILE", "staging")], || {
                panic!("local cluster should not start for an undefined requested profile")
            })
            .unwrap_err();

        assert!(err.to_string().contains("default profile does not exist"));
    }

    #[test]
    fn parses_profile_config() {
        let config = parse_config_file(
            r#"{
              "defaultProfile": "local-pg17",
              "profiles": [
                {
                  "name": "local-pg17",
                  "adminUrl": "postgres://postgres:postgres@localhost:5432/postgres",
                  "databasePrefix": "agentdb",
                  "defaultTtlMinutes": 10,
                  "maxTtlMinutes": 20
                }
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(config.default_profile, "local-pg17");
        assert_eq!(config.profiles[0].database_prefix, "agentdb");
        assert!(config.telemetry.enabled);
    }

    #[test]
    fn parses_telemetry_config() {
        let config = parse_config_file(
            r#"{
              "defaultProfile": "local",
              "profiles": [{ "name": "local", "adminUrl": "postgres://localhost/postgres" }],
              "telemetry": { "enabled": false }
            }"#,
        )
        .unwrap();

        assert!(!config.telemetry.enabled);
    }

    #[test]
    fn telemetry_can_be_disabled_from_env() {
        let config = load_config_from_env([
            (
                "PGSANDBOX_ADMIN_DATABASE_URL",
                "postgres://postgres:postgres@localhost/postgres",
            ),
            ("PGSANDBOX_TELEMETRY", "false"),
        ])
        .unwrap();

        assert!(!config.telemetry.enabled);
    }

    #[test]
    fn do_not_track_disables_telemetry() {
        let config = telemetry_config_from_env([("DO_NOT_TRACK", "1")]);

        assert!(!config.enabled);
    }

    #[test]
    fn telemetry_disable_vars_override_explicit_enable() {
        let config = telemetry_config_from_env([
            ("PGSANDBOX_TELEMETRY", "true"),
            ("PGSANDBOX_NO_TELEMETRY", "1"),
        ]);

        assert!(!config.enabled);
    }

    #[test]
    fn standalone_telemetry_loader_respects_config_file_opt_out() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pgsandbox.config.json");
        fs::write(
            &path,
            r#"{
              "defaultProfile": "local",
              "profiles": [{ "name": "local", "adminUrl": "postgres://localhost/postgres" }],
              "telemetry": { "enabled": false }
            }"#,
        )
        .unwrap();

        let config = telemetry_config_from_env([("PGSANDBOX_CONFIG", path.to_str().unwrap())]);

        assert!(!config.enabled);
    }

    #[test]
    fn telemetry_true_does_not_override_config_file_opt_out() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pgsandbox.config.json");
        fs::write(&path, r#"{ "telemetry": { "enabled": false } }"#).unwrap();

        let config = telemetry_config_from_env([
            ("PGSANDBOX_CONFIG", path.to_str().unwrap()),
            ("PGSANDBOX_TELEMETRY", "true"),
        ]);

        assert!(!config.enabled);
    }

    #[test]
    fn unreadable_config_file_disables_standalone_telemetry() {
        let config = telemetry_config_from_env([("PGSANDBOX_CONFIG", "/missing/pgsandbox.json")]);

        assert!(!config.enabled);
    }

    #[test]
    fn rejects_unknown_default_profile() {
        let err = parse_config_file(
            r#"{
              "defaultProfile": "missing",
              "profiles": [{ "name": "local", "adminUrl": "postgres://localhost/postgres" }]
            }"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("default profile"));
    }
}
