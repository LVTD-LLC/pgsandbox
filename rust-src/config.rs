use std::{env, fs};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_DATABASE_PREFIX: &str = "pgsandbox";
const DEFAULT_TTL_MINUTES: u32 = 240;
const DEFAULT_MAX_TTL_MINUTES: u32 = 1440;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Set PGSANDBOX_ADMIN_DATABASE_URL or PGSANDBOX_CONFIG before starting pgsandbox-mcp.")]
    MissingAdminUrl,
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
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxConfig {
    pub default_profile: String,
    pub profiles: Vec<SandboxProfile>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawConfig {
    default_profile: Option<String>,
    profiles: Vec<SandboxProfile>,
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
    let env = vars
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect::<std::collections::HashMap<_, _>>();

    if let Some(path) = env.get("PGSANDBOX_CONFIG") {
        return parse_config_file(&fs::read_to_string(path).map_err(|source| {
            ConfigError::ReadFile {
                path: path.clone(),
                source,
            }
        })?);
    }

    let admin_url = env
        .get("PGSANDBOX_ADMIN_DATABASE_URL")
        .ok_or(ConfigError::MissingAdminUrl)?
        .to_string();

    let name = env
        .get("PGSANDBOX_DEFAULT_PROFILE")
        .cloned()
        .unwrap_or_else(|| "default".to_string());

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
    })
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

fn normalize_config(raw: RawConfig) -> Result<SandboxConfig, ConfigError> {
    if raw.profiles.is_empty() {
        return Err(ConfigError::MissingAdminUrl);
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
    })
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
