use std::path::Path;

use url::Url;

use crate::{
    config::{admin_url_host, is_local_admin_url, load_config_from_env, SandboxConfig},
    local::discover_local_postgres_installations,
    postgres::connect_url,
    setup::{detect_existing_client_configs, find_configured_admin_url},
};

pub struct DoctorResult {
    pub ok: bool,
    pub available_postgres_versions: Vec<String>,
    pub lines: Vec<String>,
}

pub async fn run_doctor(
    admin_url: Option<&str>,
    postgres_version: Option<&str>,
    cwd: &Path,
) -> DoctorResult {
    let mut lines = vec![format!(
        "CLI: {}",
        std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "pgsandbox-mcp".to_string())
    )];
    let available_postgres_versions = discovered_postgres_versions();
    let mut ok = true;
    let mut configured_admin_url_target = None;

    let mut env = std::env::vars().collect::<Vec<_>>();
    if let Some(admin_url) = admin_url {
        env.push((
            "PGSANDBOX_ADMIN_DATABASE_URL".to_string(),
            admin_url.to_string(),
        ));
    } else if std::env::var_os("PGSANDBOX_CONFIG").is_none()
        && std::env::var_os("PGSANDBOX_ADMIN_DATABASE_URL").is_none()
    {
        if let Some((target, configured_admin_url)) = find_configured_admin_url(cwd, "pgsandbox") {
            lines.push(format!(
                "Using admin URL from {} {} MCP config.",
                target.client, target.scope
            ));
            env.push((
                "PGSANDBOX_ADMIN_DATABASE_URL".to_string(),
                configured_admin_url,
            ));
            configured_admin_url_target = Some(target);
        }
    }
    if let Some(postgres_version) = postgres_version {
        env.push((
            "PGSANDBOX_POSTGRES_VERSION".to_string(),
            postgres_version.to_string(),
        ));
    }

    let config = match load_config_from_env(env) {
        Ok(config) => Some(config),
        Err(error) => {
            ok = false;
            lines.extend(doctor_config_error_lines(
                &error.to_string(),
                &available_postgres_versions,
            ));
            None
        }
    };

    if let Some(config) = config {
        let postgres_ok = check_profiles(&config, &mut lines).await;
        ok = ok && postgres_ok;
        if !postgres_ok {
            if let Some(target) = configured_admin_url_target {
                lines.push(format!(
                    "Hint: this check used an explicit admin URL from {} {} MCP config. If you want the managed local cluster instead, run `pgsandbox-mcp setup --client {}{}` without `--admin-url`, restart the MCP client, and rerun doctor.",
                    target.client,
                    target.scope,
                    target.client,
                    if target.scope == crate::setup::ConfigScope::Project {
                        " --scope project"
                    } else {
                        ""
                    }
                ));
            }
        }
    }

    let configs = detect_existing_client_configs(cwd);
    if configs.is_empty() {
        lines.push("MCP client configs: none found yet".to_string());
    } else {
        for config in configs {
            let readable = std::fs::File::open(&config.path).is_ok();
            lines.push(format!(
                "MCP client config: {} {} {} at {}",
                config.client,
                config.scope,
                if readable { "found" } else { "unreadable" },
                config.path.display()
            ));
        }
    }

    DoctorResult {
        ok,
        available_postgres_versions,
        lines,
    }
}

fn discovered_postgres_versions() -> Vec<String> {
    let mut versions = discover_local_postgres_installations()
        .into_iter()
        .map(|installation| installation.postgres_version)
        .collect::<Vec<_>>();
    sort_postgres_versions(&mut versions);
    versions.dedup();
    versions
}

fn sort_postgres_versions(versions: &mut [String]) {
    versions.sort_by(|left, right| {
        let left_major = left.parse::<u32>().ok();
        let right_major = right.parse::<u32>().ok();
        match (left_major, right_major) {
            (Some(left), Some(right)) => left.cmp(&right),
            _ => left.cmp(right),
        }
    });
}

fn doctor_config_error_lines(message: &str, available_versions: &[String]) -> Vec<String> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("could not find local postgres") && lower.contains("binaries") {
        let requested_version = requested_postgres_version_from_message(message);
        let mut lines = vec![match requested_version.as_deref() {
            Some(version) => format!("Local Postgres {version} binaries are unavailable."),
            None => "Local Postgres binaries are unavailable.".to_string(),
        }];
        lines.push(available_versions_line(available_versions));
        lines.push(match requested_version {
            Some(version) => format!(
                "Hint: run `pgsandbox-mcp setup --client <client> --postgres-version {version}` to let setup prepare the local runtime. If setup cannot install PostgreSQL automatically, set PGSANDBOX_POSTGRES_{version}_BIN_DIR or PGSANDBOX_POSTGRES_BIN_DIR, or choose an available version."
            ),
            None => "Hint: run `pgsandbox-mcp setup --client <client>` to let setup prepare the local runtime. If setup cannot install PostgreSQL automatically, set PGSANDBOX_POSTGRES_BIN_DIR or choose an available version.".to_string(),
        });
        return lines;
    }

    vec![message.to_string()]
}

fn available_versions_line(available_versions: &[String]) -> String {
    if available_versions.is_empty() {
        "Available local Postgres versions: none discovered".to_string()
    } else {
        format!(
            "Available local Postgres versions: {}",
            available_versions.join(", ")
        )
    }
}

fn requested_postgres_version_from_message(message: &str) -> Option<String> {
    requested_version_after(message, "local Postgres")
        .or_else(|| requested_version_after(message, "postgresVersion"))
}

fn requested_version_after(message: &str, marker: &str) -> Option<String> {
    message.match_indices(marker).find_map(|(start, _)| {
        let version = message[start + marker.len()..]
            .trim_start_matches(|character: char| {
                character.is_whitespace() || matches!(character, ':' | '=' | '"' | '`')
            })
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>();
        (!version.is_empty()).then_some(version)
    })
}

async fn check_profiles(config: &SandboxConfig, lines: &mut Vec<String>) -> bool {
    let mut ok = true;
    for profile in &config.profiles {
        lines.push(format!(
            "Profile {}: {}",
            profile.name,
            mask_connection_string(&profile.admin_url)
        ));
        if matches!(is_local_admin_url(&profile.admin_url), Ok(false)) {
            lines.push(format!(
                "Profile {} policy: external admin URL explicitly enabled for host {}",
                profile.name,
                admin_url_host(&profile.admin_url)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "(unknown)".to_string())
            ));
        }
        if let Some(limit) = profile.max_active_databases_per_owner {
            lines.push(format!(
                "Profile {} policy: maxActiveDatabasesPerOwner={limit}",
                profile.name
            ));
        }
        let result = check_postgres(&profile.admin_url).await;
        ok = ok && result.0;
        lines.push(format!(
            "Postgres connection ({}): {}",
            profile.name, result.1
        ));
    }
    ok
}

async fn check_postgres(admin_url: &str) -> (bool, String) {
    let connect =
        tokio::time::timeout(std::time::Duration::from_secs(3), connect_url(admin_url)).await;

    let (client, connection_task) = match connect {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return (false, format!("{error:#}")),
        Err(_) => return (false, "connection timed out".to_string()),
    };

    let result = client.query_one("SELECT 1", &[]).await;
    drop(client);
    let _ = connection_task.await;

    match result {
        Ok(_) => (true, "ok".to_string()),
        Err(error) => (false, format!("{error:#}")),
    }
}

pub fn mask_connection_string(value: &str) -> String {
    if let Ok(mut url) = Url::parse(value) {
        if url.password().is_some() {
            let _ = url.set_password(Some("****"));
        }
        return url.to_string();
    }

    let Some((prefix, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    let Some((creds, suffix)) = rest.split_once('@') else {
        return value.to_string();
    };
    let Some((user, _password)) = creds.split_once(':') else {
        return value.to_string();
    };

    format!("{prefix}://{user}:****@{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_connection_passwords() {
        assert_eq!(
            mask_connection_string("postgres://postgres:secret@localhost:5432/postgres"),
            "postgres://postgres:****@localhost:5432/postgres"
        );
    }

    #[test]
    fn compact_local_postgres_errors_omit_probe_paths() {
        let lines = doctor_config_error_lines(
            "failed to prepare default local Postgres cluster: could not find local Postgres 99 binaries. Set PGSANDBOX_POSTGRES_99_BIN_DIR or put matching initdb, pg_ctl, and postgres on PATH. Tried: common bin dir /very/long/path/bin: failed; local bin dir /another/path/bin: resolved Postgres 18, not requested 99",
            &["16".to_string(), "18".to_string()],
        );
        let text = lines.join("\n");

        assert!(text.contains("Local Postgres 99 binaries are unavailable."));
        assert!(text.contains("Available local Postgres versions: 16, 18"));
        assert!(text.contains("pgsandbox-mcp setup --client <client> --postgres-version 99"));
        assert!(text.contains("PGSANDBOX_POSTGRES_99_BIN_DIR"));
        assert!(!text.contains("Tried:"));
        assert!(!text.contains("/very/long/path"));
        assert!(!text.contains("/another/path"));
    }

    #[test]
    fn discovered_postgres_versions_sort_numerically() {
        let mut versions = vec!["9".to_string(), "18".to_string(), "16".to_string()];

        sort_postgres_versions(&mut versions);

        assert_eq!(versions, ["9", "16", "18"]);
    }
}
