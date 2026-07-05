use std::{env, fs};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::local::{LocalClusterConfig, LocalPostgresCluster, LOCAL_PROFILE_NAME};

const DEFAULT_DATABASE_PREFIX: &str = "pgsandbox";
const DEFAULT_TTL_MINUTES: u32 = 240;
const DEFAULT_MAX_TTL_MINUTES: u32 = 1440;
pub(crate) const DEFERRED_LOCAL_ADMIN_URL: &str =
    "postgres://pgsandbox_admin:deferred@127.0.0.1:0/postgres?sslmode=disable";

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
    #[error("{0}")]
    InvalidTtl(String),
    #[error("profile {profile} adminUrl is invalid: {source}")]
    InvalidAdminUrl {
        profile: String,
        #[source]
        source: url::ParseError,
    },
    #[error("profile {profile} adminUrl host {host} is not in allowedAdminHosts")]
    AdminHostNotAllowed { profile: String, host: String },
    #[error("profile {0} adminUrl points at a non-local host; set allowExternalAdminUrl true or list the host in allowedAdminHosts to opt in explicitly")]
    ExternalAdminUrlRequiresOptIn(String),
    #[error("default profile does not exist: {0}")]
    MissingDefaultProfile(String),
    #[error("Unknown Postgres profile: {0}")]
    UnknownProfile(String),
    #[error("Unknown Postgres version: {0}")]
    UnknownPostgresVersion(String),
    #[error("Postgres version {version} matches multiple profiles: {profiles}")]
    AmbiguousPostgresVersion { version: String, profiles: String },
    #[error("profile {profile} postgresVersion {profile_version} does not match requested postgresVersion {requested_version}")]
    PostgresVersionConflict {
        profile: String,
        profile_version: String,
        requested_version: String,
    },
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
    #[serde(default)]
    pub managed_local: ManagedLocalConfig,
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
    #[serde(default)]
    pub allow_external_admin_url: bool,
    #[serde(default)]
    pub allowed_admin_hosts: Vec<String>,
    #[serde(default)]
    pub max_active_databases_per_owner: Option<u32>,
    #[serde(default)]
    pub postgres_version: Option<String>,
    #[serde(default)]
    pub managed_local: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedLocalConfig {
    #[serde(default)]
    pub enabled: bool,
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

pub fn load_config_deferred_local() -> Result<SandboxConfig, ConfigError> {
    load_config_from_env_deferred_local(env::vars())
}

pub fn load_config_from_env<I, K, V>(vars: I) -> Result<SandboxConfig, ConfigError>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    load_config_from_env_with_local(
        vars,
        |postgres_version| {
            let config = LocalPostgresCluster::from_env_for_version(postgres_version)
                .map_err(|error| ConfigError::LocalPostgres(error.to_string()))?
                .ensure_started()
                .map_err(|error| ConfigError::LocalPostgres(error.to_string()))?;
            Ok(config)
        },
        false,
    )
}

pub fn load_config_from_env_deferred_local<I, K, V>(vars: I) -> Result<SandboxConfig, ConfigError>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    load_config_from_env_with_local(
        vars,
        |_| unreachable!("deferred managed local loading must not start Postgres"),
        true,
    )
}

fn load_config_from_env_with_local<I, K, V, F>(
    vars: I,
    local_admin_url: F,
    defer_managed_local: bool,
) -> Result<SandboxConfig, ConfigError>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
    F: FnOnce(Option<&str>) -> Result<LocalClusterConfig, ConfigError>,
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
        let requested_postgres_version = env.get("PGSANDBOX_POSTGRES_VERSION").map(String::as_str);
        let explicit_default_profile = env.get("PGSANDBOX_DEFAULT_PROFILE").cloned();
        let (admin_url, name, postgres_version, managed_local_profile) =
            match env.get("PGSANDBOX_ADMIN_DATABASE_URL") {
                Some(admin_url) => (
                    admin_url.to_string(),
                    explicit_default_profile.unwrap_or_else(|| "default".to_string()),
                    requested_postgres_version.map(ToString::to_string),
                    false,
                ),
                None => {
                    let default_profile =
                        explicit_default_profile.unwrap_or_else(|| LOCAL_PROFILE_NAME.to_string());
                    if default_profile != LOCAL_PROFILE_NAME {
                        return Err(ConfigError::MissingDefaultProfile(default_profile));
                    }
                    let local = if defer_managed_local {
                        deferred_local_cluster_config(requested_postgres_version)?
                    } else {
                        local_admin_url(requested_postgres_version)?
                    };
                    (
                        local.admin_url,
                        local.profile_name,
                        local.postgres_version,
                        true,
                    )
                }
            };

        let mut config = normalize_config(RawConfig {
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
                allow_external_admin_url: env
                    .get("PGSANDBOX_ALLOW_EXTERNAL_ADMIN_URL")
                    .and_then(|value| parse_bool_flag(value))
                    .unwrap_or(false),
                allowed_admin_hosts: env
                    .get("PGSANDBOX_ALLOWED_ADMIN_HOSTS")
                    .map(|value| parse_csv_list(value))
                    .unwrap_or_default(),
                max_active_databases_per_owner: env
                    .get("PGSANDBOX_MAX_ACTIVE_DATABASES_PER_OWNER")
                    .and_then(|value| value.parse().ok()),
                postgres_version,
                managed_local: managed_local_profile,
            }],
            telemetry: TelemetryConfig::default(),
        })?;
        if managed_local_profile {
            config.managed_local.enabled = true;
        }
        config
    };

    apply_telemetry_env_overrides(&mut config.telemetry, &env);
    Ok(config)
}

fn deferred_local_cluster_config(
    requested_postgres_version: Option<&str>,
) -> Result<LocalClusterConfig, ConfigError> {
    let postgres_version = requested_postgres_version
        .map(|value| {
            let normalized = normalize_postgres_version(value);
            if normalized.is_empty() {
                Err(ConfigError::LocalPostgres(
                    "postgresVersion must start with a numeric major version".to_string(),
                ))
            } else {
                Ok(normalized)
            }
        })
        .transpose()?;
    let profile_name = postgres_version
        .as_ref()
        .map(|version| format!("local-pg{version}"))
        .unwrap_or_else(|| LOCAL_PROFILE_NAME.to_string());

    Ok(LocalClusterConfig {
        profile_name,
        postgres_version,
        postgres_bin_dir: None,
        admin_url: DEFERRED_LOCAL_ADMIN_URL.to_string(),
        host: "127.0.0.1".to_string(),
        port: 0,
        data_dir: std::path::PathBuf::new(),
        socket_dir: std::path::PathBuf::new(),
        log_file: std::path::PathBuf::new(),
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

pub fn find_profile_for_request<'a>(
    config: &'a SandboxConfig,
    profile_name: Option<&str>,
    postgres_version: Option<&str>,
) -> Result<&'a SandboxProfile, ConfigError> {
    let postgres_version = postgres_version.map(normalize_postgres_version);
    let profile = match profile_name {
        Some(profile_name) => Some(
            config
                .profiles
                .iter()
                .find(|profile| profile.name == profile_name)
                .ok_or_else(|| ConfigError::UnknownProfile(profile_name.to_string()))?,
        ),
        None => None,
    };

    match (profile, postgres_version) {
        (Some(profile), Some(requested_version)) => {
            let profile_version = profile
                .postgres_version
                .as_deref()
                .map(normalize_postgres_version);
            if profile_version.as_deref() == Some(requested_version.as_str()) {
                Ok(profile)
            } else {
                Err(ConfigError::PostgresVersionConflict {
                    profile: profile.name.clone(),
                    profile_version: profile
                        .postgres_version
                        .clone()
                        .unwrap_or_else(|| "(unspecified)".to_string()),
                    requested_version,
                })
            }
        }
        (Some(profile), None) => Ok(profile),
        (None, Some(requested_version)) => {
            let matches = config
                .profiles
                .iter()
                .filter(|profile| {
                    profile
                        .postgres_version
                        .as_deref()
                        .map(normalize_postgres_version)
                        .as_deref()
                        == Some(requested_version.as_str())
                })
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [profile] => Ok(profile),
                [] => Err(ConfigError::UnknownPostgresVersion(requested_version)),
                profiles => Err(ConfigError::AmbiguousPostgresVersion {
                    version: requested_version,
                    profiles: profiles
                        .iter()
                        .map(|profile| profile.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                }),
            }
        }
        (None, None) => find_profile(config, None),
    }
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
        if profile.default_ttl_minutes == 0 {
            return Err(ConfigError::InvalidTtl(format!(
                "defaultTtlMinutes must be at least 1 for profile {}",
                profile.name
            )));
        }
        if profile.max_ttl_minutes == 0 {
            return Err(ConfigError::InvalidTtl(format!(
                "maxTtlMinutes must be at least 1 for profile {}",
                profile.name
            )));
        }
        if profile.default_ttl_minutes > profile.max_ttl_minutes {
            return Err(ConfigError::InvalidTtl(format!(
                "defaultTtlMinutes cannot exceed maxTtlMinutes for profile {}",
                profile.name
            )));
        }
        validate_admin_url_policy(profile)?;
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
        managed_local: ManagedLocalConfig::default(),
    })
}

pub fn normalize_postgres_version(value: &str) -> String {
    value
        .trim()
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect()
}

pub fn admin_url_host(admin_url: &str) -> Result<Option<String>, url::ParseError> {
    Ok(Url::parse(admin_url)?
        .host_str()
        .map(|host| host.trim_matches(['[', ']']).to_ascii_lowercase()))
}

pub fn is_local_admin_url(admin_url: &str) -> Result<bool, url::ParseError> {
    let Some(host) = admin_url_host(admin_url)? else {
        return Ok(true);
    };
    Ok(matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1"))
}

fn validate_admin_url_policy(profile: &SandboxProfile) -> Result<(), ConfigError> {
    let host =
        admin_url_host(&profile.admin_url).map_err(|source| ConfigError::InvalidAdminUrl {
            profile: profile.name.clone(),
            source,
        })?;
    let normalized_allowed_hosts = profile
        .allowed_admin_hosts
        .iter()
        .map(|host| host.trim_matches(['[', ']']).to_ascii_lowercase())
        .collect::<Vec<_>>();

    let is_local =
        is_local_admin_url(&profile.admin_url).map_err(|source| ConfigError::InvalidAdminUrl {
            profile: profile.name.clone(),
            source,
        })?;
    if is_local {
        return Ok(());
    }

    if !normalized_allowed_hosts.is_empty() {
        let host_for_error = host.clone().unwrap_or_else(|| "(none)".to_string());
        let host_allowed = host.as_deref().is_some_and(|host| {
            normalized_allowed_hosts
                .iter()
                .any(|allowed| allowed == host)
        });
        if !host_allowed {
            return Err(ConfigError::AdminHostNotAllowed {
                profile: profile.name.clone(),
                host: host_for_error,
            });
        }
    }

    if profile.allow_external_admin_url || !normalized_allowed_hosts.is_empty() {
        return Ok(());
    }

    Err(ConfigError::ExternalAdminUrlRequiresOptIn(
        profile.name.clone(),
    ))
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

fn parse_csv_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
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
        assert!(!config.profiles[0].allow_external_admin_url);
        assert!(config.profiles[0].allowed_admin_hosts.is_empty());
        assert_eq!(config.profiles[0].max_active_databases_per_owner, None);
        assert!(config.telemetry.enabled);
    }

    #[test]
    fn loads_managed_local_profile_when_no_admin_url_is_set() {
        let mut called = false;
        let config = load_config_from_env_with_local(
            std::iter::empty::<(&str, &str)>(),
            |_| {
                called = true;
                Ok(crate::local::LocalClusterConfig {
                    profile_name: "local".to_string(),
                    admin_url:
                        "postgres://pgsandbox_admin:secret@127.0.0.1:65432/postgres?sslmode=disable"
                            .to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 65432,
                    data_dir: "/tmp/pgsandbox/postgres/data".into(),
                    socket_dir: "/tmp/pgsandbox/postgres/run".into(),
                    log_file: "/tmp/pgsandbox/postgres/postgres.log".into(),
                    postgres_version: Some("16".to_string()),
                    postgres_bin_dir: None,
                })
            },
            false,
        )
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
        assert_eq!(config.profiles[0].postgres_version.as_deref(), Some("16"));
        assert!(config.profiles[0].managed_local);
        assert!(config.managed_local.enabled);
    }

    #[test]
    fn can_load_managed_local_profile_without_starting_cluster() {
        let config = load_config_from_env_deferred_local(std::iter::empty::<(&str, &str)>())
            .expect("deferred local config should load");

        assert_eq!(config.default_profile, "local");
        assert!(config.managed_local.enabled);
        assert_eq!(config.profiles[0].name, "local");
        assert!(config.profiles[0].managed_local);
        assert_eq!(config.profiles[0].admin_url, DEFERRED_LOCAL_ADMIN_URL);
    }

    #[test]
    fn loads_requested_managed_local_postgres_version_from_env() {
        let mut requested_version = None;
        let config = load_config_from_env_with_local(
            [("PGSANDBOX_POSTGRES_VERSION", "17")],
            |version| {
                requested_version = version.map(ToString::to_string);
                Ok(crate::local::LocalClusterConfig {
                    profile_name: "local-pg17".to_string(),
                    admin_url:
                        "postgres://pgsandbox_admin:secret@127.0.0.1:65433/postgres?sslmode=disable"
                            .to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 65433,
                    data_dir: "/tmp/pgsandbox/postgres/versions/17/data".into(),
                    socket_dir: "/tmp/pgsandbox/postgres/versions/17/run".into(),
                    log_file: "/tmp/pgsandbox/postgres/versions/17/postgres.log".into(),
                    postgres_version: Some("17".to_string()),
                    postgres_bin_dir: Some("/opt/postgresql@17/bin".into()),
                })
            },
            false,
        )
        .unwrap();

        assert_eq!(requested_version.as_deref(), Some("17"));
        assert_eq!(config.default_profile, "local-pg17");
        assert_eq!(config.profiles[0].name, "local-pg17");
        assert_eq!(config.profiles[0].postgres_version.as_deref(), Some("17"));
        assert!(config.profiles[0].managed_local);
    }

    #[test]
    fn resolves_profile_by_requested_postgres_version() {
        let config = parse_config_file(
            r#"{
              "defaultProfile": "pg16",
              "profiles": [
                {
                  "name": "pg16",
                  "adminUrl": "postgres://postgres:postgres@localhost:5416/postgres",
                  "postgresVersion": "16"
                },
                {
                  "name": "pg17",
                  "adminUrl": "postgres://postgres:postgres@localhost:5417/postgres",
                  "postgresVersion": "17"
                }
              ]
            }"#,
        )
        .unwrap();

        let profile = find_profile_for_request(&config, None, Some("17")).unwrap();

        assert_eq!(profile.name, "pg17");
    }

    #[test]
    fn rejects_profile_and_postgres_version_conflict() {
        let config = parse_config_file(
            r#"{
              "defaultProfile": "pg16",
              "profiles": [
                {
                  "name": "pg16",
                  "adminUrl": "postgres://postgres:postgres@localhost:5416/postgres",
                  "postgresVersion": "16"
                }
              ]
            }"#,
        )
        .unwrap();

        let err = find_profile_for_request(&config, Some("pg16"), Some("17")).unwrap_err();

        assert!(err
            .to_string()
            .contains("does not match requested postgresVersion"));
    }

    #[test]
    fn explicit_admin_url_skips_managed_local_profile() {
        let config = load_config_from_env_with_local(
            [(
                "PGSANDBOX_ADMIN_DATABASE_URL",
                "postgres://postgres:postgres@localhost/postgres",
            )],
            |_| panic!("local cluster should not start when an admin URL is explicit"),
            false,
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
        let err = load_config_from_env_with_local(
            [("PGSANDBOX_DEFAULT_PROFILE", "staging")],
            |_| panic!("local cluster should not start for an undefined requested profile"),
            false,
        )
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
                  "maxTtlMinutes": 20,
                  "maxActiveDatabasesPerOwner": 2
                }
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(config.default_profile, "local-pg17");
        assert_eq!(config.profiles[0].database_prefix, "agentdb");
        assert_eq!(config.profiles[0].max_active_databases_per_owner, Some(2));
        assert!(config.telemetry.enabled);
    }

    #[test]
    fn rejects_non_local_admin_url_without_explicit_opt_in() {
        let err = parse_config_file(
            r#"{
              "defaultProfile": "remote",
              "profiles": [{ "name": "remote", "adminUrl": "postgres://postgres:postgres@db.example.com/postgres" }]
            }"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("non-local host"));
    }

    #[test]
    fn allows_non_local_admin_url_with_explicit_opt_in() {
        let config = parse_config_file(
            r#"{
              "defaultProfile": "remote",
              "profiles": [{
                "name": "remote",
                "adminUrl": "postgres://postgres:postgres@db.example.com/postgres",
                "allowExternalAdminUrl": true
              }]
            }"#,
        )
        .unwrap();

        assert!(config.profiles[0].allow_external_admin_url);
    }

    #[test]
    fn allowed_admin_hosts_opt_in_and_restrict_hosts() {
        let config = parse_config_file(
            r#"{
              "defaultProfile": "remote",
              "profiles": [{
                "name": "remote",
                "adminUrl": "postgres://postgres:postgres@db.example.com/postgres",
                "allowedAdminHosts": ["db.example.com"]
              }]
            }"#,
        )
        .unwrap();

        assert_eq!(config.profiles[0].allowed_admin_hosts, ["db.example.com"]);

        let err = parse_config_file(
            r#"{
              "defaultProfile": "remote",
              "profiles": [{
                "name": "remote",
                "adminUrl": "postgres://postgres:postgres@other.example.com/postgres",
                "allowedAdminHosts": ["db.example.com"]
              }]
            }"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("allowedAdminHosts"));
    }

    #[test]
    fn allowed_admin_hosts_do_not_block_hostless_local_urls() {
        let config = parse_config_file(
            r#"{
              "defaultProfile": "local-socket",
              "profiles": [{
                "name": "local-socket",
                "adminUrl": "postgres:///postgres",
                "allowedAdminHosts": ["db.example.com"]
              }]
            }"#,
        )
        .unwrap();

        assert_eq!(config.profiles[0].admin_url, "postgres:///postgres");
    }

    #[test]
    fn parses_policy_from_env() {
        let config = load_config_from_env([
            (
                "PGSANDBOX_ADMIN_DATABASE_URL",
                "postgres://postgres:postgres@db.example.com/postgres",
            ),
            ("PGSANDBOX_ALLOW_EXTERNAL_ADMIN_URL", "true"),
            (
                "PGSANDBOX_ALLOWED_ADMIN_HOSTS",
                "db.example.com, standby.example.com",
            ),
            ("PGSANDBOX_MAX_ACTIVE_DATABASES_PER_OWNER", "3"),
        ])
        .unwrap();

        let profile = &config.profiles[0];
        assert!(profile.allow_external_admin_url);
        assert_eq!(
            profile.allowed_admin_hosts,
            ["db.example.com", "standby.example.com"]
        );
        assert_eq!(profile.max_active_databases_per_owner, Some(3));
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

    #[test]
    fn rejects_zero_default_ttl_minutes() {
        let err = parse_config_file(
            r#"{
              "defaultProfile": "local",
              "profiles": [{
                "name": "local",
                "adminUrl": "postgres://localhost/postgres",
                "defaultTtlMinutes": 0,
                "maxTtlMinutes": 20
              }]
            }"#,
        )
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("defaultTtlMinutes"));
        assert!(message.contains("at least 1"));
    }

    #[test]
    fn rejects_zero_max_ttl_minutes() {
        let err = parse_config_file(
            r#"{
              "defaultProfile": "local",
              "profiles": [{
                "name": "local",
                "adminUrl": "postgres://localhost/postgres",
                "defaultTtlMinutes": 1,
                "maxTtlMinutes": 0
              }]
            }"#,
        )
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("maxTtlMinutes"));
        assert!(message.contains("at least 1"));
    }
}
