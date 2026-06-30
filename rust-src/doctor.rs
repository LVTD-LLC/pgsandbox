use std::path::Path;

use url::Url;

use crate::{
    config::{admin_url_host, is_local_admin_url, load_config_from_env, SandboxConfig},
    postgres::connect_url,
    setup::{detect_existing_client_configs, find_configured_admin_url},
};

pub struct DoctorResult {
    pub ok: bool,
    pub lines: Vec<String>,
}

pub async fn run_doctor(admin_url: Option<&str>, cwd: &Path) -> DoctorResult {
    let mut lines = vec![format!(
        "CLI: {}",
        std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "pgsandbox-mcp".to_string())
    )];
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

    let config = match load_config_from_env(env) {
        Ok(config) => Some(config),
        Err(error) => {
            ok = false;
            lines.push(error.to_string());
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

    DoctorResult { ok, lines }
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
}
