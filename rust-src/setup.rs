use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

const ADMIN_DATABASE_URL_ENV: &str = "PGSANDBOX_ADMIN_DATABASE_URL";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientSelector {
    Codex,
    ClaudeDesktop,
    Cursor,
    Vscode,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigScope {
    User,
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigFormat {
    CodexToml,
    McpJson,
    VscodeJson,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupportedClient {
    Codex,
    ClaudeDesktop,
    Cursor,
    Vscode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpLaunchConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigTarget {
    pub client: SupportedClient,
    pub scope: ConfigScope,
    pub path: PathBuf,
    pub format: ConfigFormat,
}

#[derive(Clone, Debug)]
pub struct WriteResult {
    pub target: ConfigTarget,
    pub action: &'static str,
    pub content: String,
}

#[derive(Debug, Error)]
pub enum SetupError {
    #[error("Unsupported client: {0}")]
    UnsupportedClient(String),
    #[error("Unsupported scope: {0}")]
    UnsupportedScope(String),
    #[error("Claude Desktop only supports user-scoped MCP configuration.")]
    ClaudeDesktopProjectScope,
    #[error("could not resolve home directory")]
    MissingHome,
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse JSON config {path}: {source}")]
    ParseJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl std::fmt::Display for SupportedClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            SupportedClient::Codex => "codex",
            SupportedClient::ClaudeDesktop => "claude-desktop",
            SupportedClient::Cursor => "cursor",
            SupportedClient::Vscode => "vscode",
        })
    }
}

impl std::fmt::Display for ConfigScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            ConfigScope::User => "user",
            ConfigScope::Project => "project",
        })
    }
}

pub fn parse_client(value: &str) -> Result<ClientSelector, SetupError> {
    match value {
        "codex" => Ok(ClientSelector::Codex),
        "claude-desktop" => Ok(ClientSelector::ClaudeDesktop),
        "cursor" => Ok(ClientSelector::Cursor),
        "vscode" => Ok(ClientSelector::Vscode),
        "all" => Ok(ClientSelector::All),
        other => Err(SetupError::UnsupportedClient(other.to_string())),
    }
}

pub fn parse_scope(value: &str) -> Result<ConfigScope, SetupError> {
    match value {
        "user" => Ok(ConfigScope::User),
        "project" => Ok(ConfigScope::Project),
        other => Err(SetupError::UnsupportedScope(other.to_string())),
    }
}

pub fn build_launch_config(
    name: Option<&str>,
    command: Option<&str>,
    admin_url: Option<&str>,
) -> McpLaunchConfig {
    let env = admin_url.map(|admin_url| {
        BTreeMap::from([(ADMIN_DATABASE_URL_ENV.to_string(), admin_url.to_string())])
    });

    McpLaunchConfig {
        name: name.unwrap_or("pgsandbox").to_string(),
        command: command.unwrap_or("pgsandbox-mcp").to_string(),
        args: vec!["stdio".to_string()],
        env,
    }
}

pub fn resolve_targets(
    client: ClientSelector,
    scope: ConfigScope,
    cwd: &Path,
) -> Result<Vec<ConfigTarget>, SetupError> {
    let clients = match client {
        ClientSelector::All => vec![
            SupportedClient::Codex,
            SupportedClient::ClaudeDesktop,
            SupportedClient::Cursor,
            SupportedClient::Vscode,
        ],
        ClientSelector::Codex => vec![SupportedClient::Codex],
        ClientSelector::ClaudeDesktop => vec![SupportedClient::ClaudeDesktop],
        ClientSelector::Cursor => vec![SupportedClient::Cursor],
        ClientSelector::Vscode => vec![SupportedClient::Vscode],
    };

    clients
        .into_iter()
        .map(|client| target_for_client(client, scope, cwd))
        .collect()
}

pub fn write_client_config(
    target: &ConfigTarget,
    launch: &McpLaunchConfig,
    dry_run: bool,
) -> Result<WriteResult, SetupError> {
    let existed = target.path.exists();
    let content = next_config_content(target, launch)?;

    if !dry_run {
        if let Some(parent) = target.path.parent() {
            fs::create_dir_all(parent).map_err(|source| SetupError::WriteFile {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&target.path, &content).map_err(|source| SetupError::WriteFile {
            path: target.path.clone(),
            source,
        })?;
    }

    Ok(WriteResult {
        target: target.clone(),
        action: match (dry_run, existed) {
            (true, true) => "would_update",
            (true, false) => "would_create",
            (false, true) => "updated",
            (false, false) => "created",
        },
        content,
    })
}

pub fn config_snippet(target: &ConfigTarget, launch: &McpLaunchConfig) -> String {
    if target.format == ConfigFormat::CodexToml {
        return codex_toml_block(launch);
    }

    let root_key = if target.format == ConfigFormat::VscodeJson {
        "servers"
    } else {
        "mcpServers"
    };
    serde_json::to_string_pretty(&json!({
        root_key: {
            launch.name.clone(): json_entry_for_target(target, launch)
        }
    }))
    .expect("config snippet is serializable")
}

pub fn detect_existing_client_configs(cwd: &Path) -> Vec<ConfigTarget> {
    let mut targets = Vec::new();
    for client in [
        SupportedClient::Codex,
        SupportedClient::ClaudeDesktop,
        SupportedClient::Cursor,
        SupportedClient::Vscode,
    ] {
        let scopes = if client == SupportedClient::ClaudeDesktop {
            vec![ConfigScope::User]
        } else {
            vec![ConfigScope::Project, ConfigScope::User]
        };
        for scope in scopes {
            if let Ok(target) = target_for_client(client, scope, cwd) {
                if target.path.exists() {
                    targets.push(target);
                }
            }
        }
    }
    targets
}

pub fn find_configured_admin_url(cwd: &Path, server_name: &str) -> Option<(ConfigTarget, String)> {
    for target in detect_existing_client_configs(cwd) {
        let Ok(content) = fs::read_to_string(&target.path) else {
            continue;
        };
        let admin_url = match target.format {
            ConfigFormat::CodexToml => admin_url_from_codex_toml(&content, server_name),
            ConfigFormat::McpJson | ConfigFormat::VscodeJson => {
                admin_url_from_json_config(&content, target.format, server_name)
            }
        };
        if let Some(admin_url) = admin_url {
            return Some((target, admin_url));
        }
    }
    None
}

fn next_config_content(
    target: &ConfigTarget,
    launch: &McpLaunchConfig,
) -> Result<String, SetupError> {
    if target.format == ConfigFormat::CodexToml {
        let existing = read_optional(&target.path)?;
        return Ok(upsert_codex_toml(&existing, launch));
    }

    let mut existing = read_json_object(&target.path)?;
    let root_key = if target.format == ConfigFormat::VscodeJson {
        "servers"
    } else {
        "mcpServers"
    };
    let mut root = existing
        .get(root_key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    root.insert(launch.name.clone(), json_entry_for_target(target, launch));
    existing[root_key] = Value::Object(root);

    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&existing).expect("config JSON is serializable")
    ))
}

fn target_for_client(
    client: SupportedClient,
    scope: ConfigScope,
    cwd: &Path,
) -> Result<ConfigTarget, SetupError> {
    if client == SupportedClient::ClaudeDesktop && scope == ConfigScope::Project {
        return Err(SetupError::ClaudeDesktopProjectScope);
    }

    if scope == ConfigScope::Project {
        return Ok(match client {
            SupportedClient::Codex => ConfigTarget {
                client,
                scope,
                path: cwd.join(".codex/config.toml"),
                format: ConfigFormat::CodexToml,
            },
            SupportedClient::Cursor => ConfigTarget {
                client,
                scope,
                path: cwd.join(".cursor/mcp.json"),
                format: ConfigFormat::McpJson,
            },
            SupportedClient::Vscode => ConfigTarget {
                client,
                scope,
                path: cwd.join(".vscode/mcp.json"),
                format: ConfigFormat::VscodeJson,
            },
            SupportedClient::ClaudeDesktop => unreachable!(),
        });
    }

    let home = dirs::home_dir().ok_or(SetupError::MissingHome)?;
    let os = std::env::consts::OS;

    Ok(match client {
        SupportedClient::Codex => ConfigTarget {
            client,
            scope,
            path: home.join(".codex/config.toml"),
            format: ConfigFormat::CodexToml,
        },
        SupportedClient::Cursor => ConfigTarget {
            client,
            scope,
            path: home.join(".cursor/mcp.json"),
            format: ConfigFormat::McpJson,
        },
        SupportedClient::ClaudeDesktop => ConfigTarget {
            client,
            scope,
            path: claude_desktop_config_path(&home, os),
            format: ConfigFormat::McpJson,
        },
        SupportedClient::Vscode => ConfigTarget {
            client,
            scope,
            path: vscode_user_config_path(&home, os),
            format: ConfigFormat::VscodeJson,
        },
    })
}

fn claude_desktop_config_path(home: &Path, os: &str) -> PathBuf {
    match os {
        "macos" => home.join("Library/Application Support/Claude/claude_desktop_config.json"),
        "windows" => std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join("Claude/claude_desktop_config.json"),
        _ => home.join(".config/Claude/claude_desktop_config.json"),
    }
}

fn vscode_user_config_path(home: &Path, os: &str) -> PathBuf {
    match os {
        "macos" => home.join("Library/Application Support/Code/User/mcp.json"),
        "windows" => std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join("Code/User/mcp.json"),
        _ => home.join(".config/Code/User/mcp.json"),
    }
}

fn json_entry_for_target(target: &ConfigTarget, launch: &McpLaunchConfig) -> Value {
    let mut entry = serde_json::Map::from_iter([
        ("command".to_string(), Value::String(launch.command.clone())),
        (
            "args".to_string(),
            Value::Array(launch.args.iter().cloned().map(Value::String).collect()),
        ),
    ]);

    if target.format == ConfigFormat::VscodeJson {
        entry.insert("type".to_string(), Value::String("stdio".to_string()));
    }

    if let Some(env) = &launch.env {
        entry.insert(
            "env".to_string(),
            Value::Object(
                env.iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            ),
        );
    }

    Value::Object(entry)
}

fn upsert_codex_toml(existing: &str, launch: &McpLaunchConfig) -> String {
    let block = codex_toml_block(launch).trim_end().to_string();
    let mut lines = existing.lines().map(str::to_string).collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| is_codex_server_header(line, &launch.name));

    let Some(start) = start else {
        let prefix = if existing.trim().is_empty() {
            String::new()
        } else {
            format!("{}\n\n", existing.trim_end())
        };
        return format!("{prefix}{block}\n");
    };

    let mut end = start + 1;
    while end < lines.len() && !lines[end].trim_start().starts_with('[') {
        end += 1;
    }

    lines.splice(start..end, block.lines().map(str::to_string));
    format!("{}\n", lines.join("\n").trim_end())
}

fn codex_toml_block(launch: &McpLaunchConfig) -> String {
    let mut lines = vec![
        format!("[mcp_servers.{}]", toml_key(&launch.name)),
        format!("command = {}", toml_string(&launch.command)),
        format!(
            "args = [{}]",
            launch
                .args
                .iter()
                .map(|arg| toml_string(arg))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ];

    if let Some(env) = &launch.env {
        if !env.is_empty() {
            let entries = env
                .iter()
                .map(|(key, value)| format!("{} = {}", toml_key(key), toml_string(value)))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("env = {{ {entries} }}"));
        }
    }

    format!("{}\n", lines.join("\n"))
}

fn admin_url_from_codex_toml(content: &str, server_name: &str) -> Option<String> {
    let parsed = toml::from_str::<toml::Table>(content).ok()?;
    parsed
        .get("mcp_servers")?
        .get(server_name)?
        .get("env")?
        .get(ADMIN_DATABASE_URL_ENV)?
        .as_str()
        .map(ToOwned::to_owned)
}

fn admin_url_from_json_config(
    content: &str,
    format: ConfigFormat,
    server_name: &str,
) -> Option<String> {
    if content.trim().is_empty() {
        return None;
    }
    let parsed = serde_json::from_str::<Value>(content).ok()?;
    let root_key = if format == ConfigFormat::VscodeJson {
        "servers"
    } else {
        "mcpServers"
    };
    parsed
        .get(root_key)?
        .get(server_name)?
        .get("env")?
        .get(ADMIN_DATABASE_URL_ENV)?
        .as_str()
        .map(ToOwned::to_owned)
}

fn read_optional(path: &Path) -> Result<String, SetupError> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(SetupError::ReadFile {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn read_json_object(path: &Path) -> Result<Value, SetupError> {
    let content = read_optional(path)?;
    if content.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&content).map_err(|source| SetupError::ParseJson {
        path: path.to_path_buf(),
        source,
    })
}

fn is_codex_server_header(line: &str, name: &str) -> bool {
    let trimmed = line.trim();
    trimmed == format!("[mcp_servers.{name}]")
        || trimmed == format!("[mcp_servers.{}]", toml_key(name))
}

fn toml_key(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
    {
        value.to_string()
    } else {
        toml_string(value)
    }
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn writes_cursor_compatible_json() {
        let dir = tempdir().unwrap();
        let target = resolve_targets(ClientSelector::Cursor, ConfigScope::Project, dir.path())
            .unwrap()
            .remove(0);
        let launch = build_launch_config(
            None,
            None,
            Some("postgres://postgres:secret@localhost:5432/postgres"),
        );

        write_client_config(&target, &launch, false).unwrap();
        let parsed =
            serde_json::from_str::<Value>(&fs::read_to_string(&target.path).unwrap()).unwrap();

        assert_eq!(
            write_client_config(&target, &launch, true).unwrap().action,
            "would_update"
        );
        assert_eq!(
            parsed["mcpServers"]["pgsandbox"]["command"],
            "pgsandbox-mcp"
        );
        assert_eq!(
            parsed["mcpServers"]["pgsandbox"]["env"]["PGSANDBOX_ADMIN_DATABASE_URL"],
            "postgres://postgres:secret@localhost:5432/postgres"
        );
    }

    #[test]
    fn reports_created_for_new_config_files() {
        let dir = tempdir().unwrap();
        let target = resolve_targets(ClientSelector::Cursor, ConfigScope::Project, dir.path())
            .unwrap()
            .remove(0);
        let launch = build_launch_config(None, None, None);

        assert_eq!(
            write_client_config(&target, &launch, true).unwrap().action,
            "would_create"
        );
        assert_eq!(
            write_client_config(&target, &launch, false).unwrap().action,
            "created"
        );
        assert_eq!(
            write_client_config(&target, &launch, false).unwrap().action,
            "updated"
        );
    }

    #[test]
    fn upserts_codex_toml_without_deleting_other_config() {
        let dir = tempdir().unwrap();
        let target = resolve_targets(ClientSelector::Codex, ConfigScope::Project, dir.path())
            .unwrap()
            .remove(0);
        fs::create_dir_all(target.path.parent().unwrap()).unwrap();
        fs::write(
            &target.path,
            "model = \"gpt-5\"\n\n[mcp_servers.existing]\ncommand = \"other\"\n",
        )
        .unwrap();

        let launch = build_launch_config(None, Some("/opt/bin/pgsandbox-mcp"), None);
        write_client_config(&target, &launch, false).unwrap();
        let content = fs::read_to_string(target.path).unwrap();

        assert!(content.contains("model = \"gpt-5\""));
        assert!(content.contains("[mcp_servers.existing]"));
        assert!(content.contains("[mcp_servers.pgsandbox]"));
        assert!(content.contains("command = \"/opt/bin/pgsandbox-mcp\""));
    }

    #[test]
    fn setup_without_admin_url_removes_stale_codex_env() {
        let dir = tempdir().unwrap();
        let target = resolve_targets(ClientSelector::Codex, ConfigScope::Project, dir.path())
            .unwrap()
            .remove(0);
        let external_launch = build_launch_config(
            None,
            None,
            Some("postgres://postgres:secret@localhost:5432/postgres"),
        );
        write_client_config(&target, &external_launch, false).unwrap();

        let local_launch = build_launch_config(None, None, None);
        write_client_config(&target, &local_launch, false).unwrap();
        let content = fs::read_to_string(&target.path).unwrap();

        assert!(content.contains("[mcp_servers.pgsandbox]"));
        assert!(!content.contains(ADMIN_DATABASE_URL_ENV));
        assert!(admin_url_from_codex_toml(&content, "pgsandbox").is_none());
    }

    #[test]
    fn setup_without_admin_url_removes_stale_json_env() {
        let dir = tempdir().unwrap();
        let target = resolve_targets(ClientSelector::Cursor, ConfigScope::Project, dir.path())
            .unwrap()
            .remove(0);
        let external_launch = build_launch_config(
            None,
            None,
            Some("postgres://postgres:secret@localhost:5432/postgres"),
        );
        write_client_config(&target, &external_launch, false).unwrap();

        let local_launch = build_launch_config(None, None, None);
        write_client_config(&target, &local_launch, false).unwrap();
        let parsed =
            serde_json::from_str::<Value>(&fs::read_to_string(&target.path).unwrap()).unwrap();

        assert!(parsed["mcpServers"]["pgsandbox"]["env"].is_null());
        assert!(admin_url_from_json_config(
            &fs::read_to_string(&target.path).unwrap(),
            target.format,
            "pgsandbox"
        )
        .is_none());
    }

    #[test]
    fn finds_admin_url_from_generated_codex_config() {
        let dir = tempdir().unwrap();
        let target = resolve_targets(ClientSelector::Codex, ConfigScope::Project, dir.path())
            .unwrap()
            .remove(0);
        let launch = build_launch_config(
            None,
            None,
            Some("postgres://postgres:secret@localhost:5432/postgres"),
        );
        write_client_config(&target, &launch, false).unwrap();

        let configured = find_configured_admin_url(dir.path(), "pgsandbox").unwrap();

        assert_eq!(configured.0.path, target.path);
        assert_eq!(
            configured.1,
            "postgres://postgres:secret@localhost:5432/postgres"
        );
    }

    #[test]
    fn finds_admin_url_from_escaped_codex_toml() {
        let content = r#"
          [mcp_servers."pg sandbox"]
          command = "pgsandbox-mcp"
          args = ["stdio"]
          env = { PGSANDBOX_ADMIN_DATABASE_URL = "postgres://postgres:\u0073ecret@localhost/postgres" }
        "#;

        assert_eq!(
            admin_url_from_codex_toml(content, "pg sandbox").unwrap(),
            "postgres://postgres:secret@localhost/postgres"
        );
    }
}
