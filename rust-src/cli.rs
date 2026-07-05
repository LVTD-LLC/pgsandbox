use std::collections::BTreeMap;

use anyhow::Context;

use crate::{
    config::{load_config, load_config_deferred_local, load_config_from_env},
    doctor::{mask_connection_string, run_doctor},
    local::{LocalClusterConfig, LocalClusterStatus, LocalPostgresCluster},
    mcp::serve_stdio,
    postgres::{CreateDatabaseInput, DatabaseSelector, PostgresSandboxManager, RunSqlInput},
    setup::{
        build_launch_config, config_snippet, parse_client, parse_scope, resolve_targets,
        write_client_config,
    },
    telemetry::{properties, Telemetry},
    VERSION,
};

pub async fn run(args: Vec<String>) -> anyhow::Result<u8> {
    let (command, rest) = args
        .split_first()
        .map(|(command, rest)| (command.as_str(), rest.to_vec()))
        .unwrap_or(("stdio", Vec::new()));

    match command {
        "stdio" => start_server().await.map(|()| 0),
        "--help" | "-h" | "help" => {
            print_help();
            Ok(0)
        }
        "--version" | "-v" | "version" => {
            println!("{VERSION}");
            Ok(0)
        }
        "setup" if has_help_flag(&rest) => {
            print_help();
            Ok(0)
        }
        "doctor" if has_help_flag(&rest) => {
            print_help();
            Ok(0)
        }
        "local" if has_help_flag(&rest) => {
            print_help();
            Ok(0)
        }
        "smoke-test" if has_help_flag(&rest) => {
            print_help();
            Ok(0)
        }
        "setup" => setup(&rest).await,
        "doctor" => doctor(&rest).await,
        "local" => local(&rest).await,
        "smoke-test" => smoke_test(&rest).await,
        "" => start_server().await.map(|()| 0),
        other => anyhow::bail!("Unknown command: {other}"),
    }
}

async fn start_server() -> anyhow::Result<()> {
    serve_stdio(load_config_deferred_local()?).await
}

async fn setup(args: &[String]) -> anyhow::Result<u8> {
    let started = std::time::Instant::now();
    let telemetry = Telemetry::new(crate::config::load_telemetry_config());
    let options = parse_options(args)?;
    let client = parse_client(options.get("client").map(String::as_str).unwrap_or("codex"))?;
    let scope = parse_scope(options.get("scope").map(String::as_str).unwrap_or("user"))?;
    let admin_url = options.get("admin-url").map(String::as_str);
    let launch = build_launch_config(
        options.get("name").map(String::as_str),
        options.get("command").map(String::as_str),
        admin_url,
        options.get("postgres-version").map(String::as_str),
    );
    let dry_run = options.contains_key("dry-run");
    let cwd = std::env::current_dir()?;
    let targets = resolve_targets(client, scope, &cwd)?;

    if admin_url.is_none() {
        eprintln!("No PGSANDBOX_ADMIN_DATABASE_URL was written. The MCP server will use the managed local Postgres cluster by default.");
    }

    for target in targets {
        let result = write_client_config(&target, &launch, dry_run)?;
        println!(
            "{}: {} {} {}",
            result.action,
            result.target.client,
            result.target.scope,
            result.target.path.display()
        );
        if dry_run {
            println!("{}", config_snippet(&result.target, &launch));
        }
    }

    println!("Next: restart the MCP client, then run `pgsandbox-mcp doctor`.");
    telemetry
        .capture(
            crate::telemetry::EVENT_CLI_COMMAND_COMPLETED,
            properties([
                ("command", serde_json::json!("setup")),
                ("client", serde_json::json!(client_selector_name(client))),
                ("scope", serde_json::json!(scope.to_string())),
                ("dryRun", serde_json::json!(dry_run)),
                ("hasAdminUrl", serde_json::json!(admin_url.is_some())),
                ("success", serde_json::json!(true)),
                (
                    "elapsedMs",
                    serde_json::json!(started.elapsed().as_millis()),
                ),
            ]),
        )
        .await;
    Ok(0)
}

async fn doctor(args: &[String]) -> anyhow::Result<u8> {
    let started = std::time::Instant::now();
    let telemetry = Telemetry::new(crate::config::load_telemetry_config());
    let options = parse_options(args)?;
    let cwd = std::env::current_dir()?;
    let result = run_doctor(
        options.get("admin-url").map(String::as_str),
        options.get("postgres-version").map(String::as_str),
        &cwd,
    )
    .await;
    for line in result.lines {
        println!("{line}");
    }
    let code = if result.ok { 0 } else { 1 };
    telemetry
        .capture(
            crate::telemetry::EVENT_CLI_COMMAND_COMPLETED,
            properties([
                ("command", serde_json::json!("doctor")),
                (
                    "hasAdminUrl",
                    serde_json::json!(options.contains_key("admin-url")),
                ),
                ("success", serde_json::json!(result.ok)),
                (
                    "elapsedMs",
                    serde_json::json!(started.elapsed().as_millis()),
                ),
            ]),
        )
        .await;
    Ok(code)
}

async fn local(args: &[String]) -> anyhow::Result<u8> {
    let command = parse_local_command(args)?;
    let cluster = LocalPostgresCluster::from_env_for_version(command.postgres_version.as_deref())?;

    match command.action {
        LocalAction::Init => {
            let config = cluster.init()?;
            println!("Local Postgres: initialized");
            print_local_config(&config, &cluster);
        }
        LocalAction::Start => {
            let config = cluster.start()?;
            println!("Local Postgres: running");
            print_local_config(&config, &cluster);
        }
        LocalAction::Stop => {
            cluster.stop()?;
            println!("Local Postgres: stopped");
        }
        LocalAction::Status => {
            print_local_status(&cluster.status()?, &cluster);
        }
    }

    Ok(0)
}

async fn smoke_test(args: &[String]) -> anyhow::Result<u8> {
    let started = std::time::Instant::now();
    let options = parse_options(args)?;
    let env = if options.contains_key("admin-url") || options.contains_key("postgres-version") {
        let mut env = std::env::vars().collect::<Vec<_>>();
        if let Some(admin_url) = options.get("admin-url") {
            env.push((
                "PGSANDBOX_ADMIN_DATABASE_URL".to_string(),
                admin_url.to_string(),
            ));
        }
        if let Some(postgres_version) = options.get("postgres-version") {
            env.push((
                "PGSANDBOX_POSTGRES_VERSION".to_string(),
                postgres_version.to_string(),
            ));
        }
        Some(env)
    } else {
        None
    };
    let config = match env {
        Some(env) => load_config_from_env(env)?,
        None => load_config()?,
    };
    let telemetry = Telemetry::new(config.telemetry.clone());
    let manager = PostgresSandboxManager::new(config);
    let mut database_id = None;

    let result = async {
        let created = manager
            .create_database(CreateDatabaseInput {
                profile: None,
                postgres_version: None,
                name_hint: Some("smoke test".to_string()),
                ttl_minutes: Some(15),
                owner: Some("smoke".to_string()),
                labels: None,
            })
            .await?;
        println!("Created sandbox: {}", created.database_name);
        database_id = Some(created.database_id.clone());

        manager
            .run_sql(RunSqlInput {
                profile: None,
                postgres_version: None,
                database_id: database_id.clone(),
                database_name: None,
                sql: "create table items(id serial primary key, name text not null, price numeric(10,2) not null, ratio numeric(12,8) not null, payload bytea not null, starts_at time not null, starts_at_tz timetz not null)".to_string(),
                readonly: Some(false),
                row_limit: None,
            })
            .await?;
        println!("Created smoke-test table");

        let inserted = manager
            .run_sql(RunSqlInput {
                profile: None,
                postgres_version: None,
                database_id: database_id.clone(),
                database_name: None,
                sql: "insert into items(name, price, ratio, payload, starts_at, starts_at_tz) values ('alpha', 12.34, 0.00000012, decode('cafe', 'hex'), time '12:34:56', timetz '12:34:56-05') returning id, name, price, ratio, payload, starts_at, starts_at_tz".to_string(),
                readonly: Some(false),
                row_limit: None,
            })
            .await?;
        anyhow::ensure!(
            inserted
                .rows
                .first()
                .and_then(|row| row.get("name"))
                .and_then(|value| value.as_str())
                == Some("alpha"),
            "INSERT ... RETURNING did not return the inserted row"
        );
        anyhow::ensure!(
            inserted
                .rows
                .first()
                .and_then(|row| row.get("price"))
                .and_then(|value| value.as_str())
                == Some("12.34"),
            "NUMERIC value did not serialize as an exact string"
        );
        anyhow::ensure!(
            inserted
                .rows
                .first()
                .and_then(|row| row.get("ratio"))
                .and_then(|value| value.as_str())
                == Some("0.00000012"),
            "small NUMERIC value did not serialize with leading fractional zeros"
        );
        anyhow::ensure!(
            inserted
                .rows
                .first()
                .and_then(|row| row.get("payload"))
                .and_then(|value| value.as_str())
                == Some("\\xcafe"),
            "BYTEA value did not serialize as hex"
        );
        anyhow::ensure!(
            inserted
                .rows
                .first()
                .and_then(|row| row.get("starts_at"))
                .and_then(|value| value.as_str())
                == Some("12:34:56"),
            "TIME value did not serialize as text"
        );
        anyhow::ensure!(
            inserted
                .rows
                .first()
                .and_then(|row| row.get("starts_at_tz"))
                .and_then(|value| value.as_str())
                == Some("12:34:56-05:00"),
            "TIMETZ value did not serialize as text"
        );
        println!("{}", serde_json::to_string_pretty(&inserted)?);

        let inserted_with_comment = manager
            .run_sql(RunSqlInput {
                profile: None,
                postgres_version: None,
                database_id: database_id.clone(),
                database_name: None,
                sql: "insert into items(name, price, ratio, payload, starts_at, starts_at_tz) values ('beta', 45.67, 0.00000034, decode('beef', 'hex'), time '01:02:03', timetz '01:02:03+02') returning id, name -- agent note".to_string(),
                readonly: Some(false),
                row_limit: None,
            })
            .await?;
        anyhow::ensure!(
            inserted_with_comment
                .rows
                .first()
                .and_then(|row| row.get("name"))
                .and_then(|value| value.as_str())
                == Some("beta"),
            "INSERT ... RETURNING with a trailing line comment did not return the inserted row"
        );
        println!("{}", serde_json::to_string_pretty(&inserted_with_comment)?);

        let updated = manager
            .run_sql(RunSqlInput {
                profile: None,
                postgres_version: None,
                database_id: database_id.clone(),
                database_name: None,
                sql: "update items set name = 'not returning' where id = 1".to_string(),
                readonly: Some(false),
                row_limit: None,
            })
            .await?;
        anyhow::ensure!(
            updated.affected_row_count == Some(1) && updated.rows.is_empty(),
            "DML with 'returning' inside a string literal was not handled as a direct query"
        );
        println!("{}", serde_json::to_string_pretty(&updated)?);

        let query = manager
            .run_sql(RunSqlInput {
                profile: None,
                postgres_version: None,
                database_id: database_id.clone(),
                database_name: None,
                sql: "select * from items where id = 1".to_string(),
                readonly: Some(true),
                row_limit: None,
            })
            .await?;
        anyhow::ensure!(
            query
                .rows
                .first()
                .and_then(|row| row.get("name"))
                .and_then(|value| value.as_str())
                == Some("not returning"),
            "SELECT query did not return the updated row"
        );
        anyhow::ensure!(
            query
                .rows
                .first()
                .and_then(|row| row.get("price"))
                .and_then(|value| value.as_str())
                == Some("12.34"),
            "SELECT query did not preserve the NUMERIC value"
        );
        anyhow::ensure!(
            query
                .rows
                .first()
                .and_then(|row| row.get("ratio"))
                .and_then(|value| value.as_str())
                == Some("0.00000012"),
            "SELECT query did not preserve the small NUMERIC value"
        );
        anyhow::ensure!(
            query
                .rows
                .first()
                .and_then(|row| row.get("payload"))
                .and_then(|value| value.as_str())
                == Some("\\xcafe"),
            "SELECT query did not preserve the BYTEA value"
        );
        anyhow::ensure!(
            query
                .rows
                .first()
                .and_then(|row| row.get("starts_at"))
                .and_then(|value| value.as_str())
                == Some("12:34:56"),
            "SELECT query did not preserve the TIME value"
        );
        anyhow::ensure!(
            query
                .rows
                .first()
                .and_then(|row| row.get("starts_at_tz"))
                .and_then(|value| value.as_str())
                == Some("12:34:56-05:00"),
            "SELECT query did not preserve the TIMETZ value"
        );
        println!("{}", serde_json::to_string_pretty(&query)?);

        let readonly_literal = manager
            .run_sql(RunSqlInput {
                profile: None,
                postgres_version: None,
                database_id: database_id.clone(),
                database_name: None,
                sql: "select 'rollback' as stage".to_string(),
                readonly: Some(true),
                row_limit: None,
            })
            .await?;
        anyhow::ensure!(
            readonly_literal
                .rows
                .first()
                .and_then(|row| row.get("stage"))
                .and_then(|value| value.as_str())
                == Some("rollback"),
            "readonly guard rejected or altered a safe string literal"
        );
        println!("{}", serde_json::to_string_pretty(&readonly_literal)?);

        manager
            .delete_database(DatabaseSelector {
                profile: None,
                postgres_version: None,
                database_id: database_id.clone(),
                database_name: None,
            })
            .await?;
        println!("Cleanup: deleted");
        database_id = None;
        anyhow::Ok(())
    }
    .await;

    if let Some(database_id) = database_id {
        let _ = manager
            .delete_database(DatabaseSelector {
                profile: None,
                postgres_version: None,
                database_id: Some(database_id),
                database_name: None,
            })
            .await;
    }

    let success = result.is_ok();
    telemetry
        .capture(
            crate::telemetry::EVENT_CLI_COMMAND_COMPLETED,
            properties([
                ("command", serde_json::json!("smoke-test")),
                (
                    "hasAdminUrl",
                    serde_json::json!(options.contains_key("admin-url")),
                ),
                ("success", serde_json::json!(success)),
                (
                    "elapsedMs",
                    serde_json::json!(started.elapsed().as_millis()),
                ),
            ]),
        )
        .await;
    result?;
    Ok(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalAction {
    Init,
    Start,
    Stop,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalCommand {
    action: LocalAction,
    postgres_version: Option<String>,
}

fn parse_local_command(args: &[String]) -> anyhow::Result<LocalCommand> {
    let (action, rest) = args
        .split_first()
        .map(|(action, rest)| (action.as_str(), rest))
        .unwrap_or(("status", &[]));

    let action = match action {
        "init" => LocalAction::Init,
        "start" => LocalAction::Start,
        "stop" => LocalAction::Stop,
        "status" => LocalAction::Status,
        other => anyhow::bail!("Unknown local command: {other}"),
    };
    let options = parse_options(rest)?;

    Ok(LocalCommand {
        action,
        postgres_version: options.get("postgres-version").cloned(),
    })
}

fn parse_options(args: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    let mut options = BTreeMap::new();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];

        match arg.as_str() {
            "--dry-run" => {
                options.insert("dry-run".to_string(), "true".to_string());
                index += 1;
            }
            "-c" => {
                let value = next_value(args, index + 1, arg)?;
                options.insert("client".to_string(), value.to_string());
                index += 2;
            }
            "-s" => {
                let value = next_value(args, index + 1, arg)?;
                options.insert("scope".to_string(), value.to_string());
                index += 2;
            }
            _ if arg.starts_with("--") => {
                let raw = &arg[2..];
                if let Some((name, value)) = raw.split_once('=') {
                    options.insert(name.to_string(), value.to_string());
                    index += 1;
                } else {
                    let value = next_value(args, index + 1, arg)?;
                    options.insert(raw.to_string(), value.to_string());
                    index += 2;
                }
            }
            _ => anyhow::bail!("Unexpected argument: {arg}"),
        }
    }

    Ok(options)
}

fn next_value<'a>(args: &'a [String], index: usize, flag: &str) -> anyhow::Result<&'a str> {
    args.get(index)
        .map(String::as_str)
        .with_context(|| format!("Missing value for {flag}"))
}

fn has_help_flag(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "help"))
}

fn client_selector_name(client: crate::setup::ClientSelector) -> &'static str {
    match client {
        crate::setup::ClientSelector::Codex => "codex",
        crate::setup::ClientSelector::ClaudeDesktop => "claude-desktop",
        crate::setup::ClientSelector::Cursor => "cursor",
        crate::setup::ClientSelector::Vscode => "vscode",
        crate::setup::ClientSelector::All => "all",
    }
}

fn print_local_status(status: &LocalClusterStatus, cluster: &LocalPostgresCluster) {
    if !status.initialized {
        println!("Local Postgres: not initialized");
        println!("Root: {}", cluster.root().display());
        println!("Next: pgsandbox-mcp local start");
        return;
    }

    println!(
        "Local Postgres: {}",
        if status.running { "running" } else { "stopped" }
    );
    if let Some(config) = &status.config {
        print_local_config(config, cluster);
    }
}

fn print_local_config(config: &LocalClusterConfig, cluster: &LocalPostgresCluster) {
    println!("Profile: {}", config.profile_name);
    println!("Data dir: {}", config.data_dir.display());
    println!("Socket dir: {}", config.socket_dir.display());
    println!("Port: {}", config.port);
    println!("Config: {}", cluster.config_path().display());
    println!("Admin URL: {}", mask_connection_string(&config.admin_url));
}

fn print_help() {
    println!("{}", help_text());
}

fn help_text() -> String {
    format!(
        r#"pgsandbox-mcp {VERSION}

Usage:
  pgsandbox-mcp                      Start the MCP server over stdio
  pgsandbox-mcp stdio                Start the MCP server over stdio
  pgsandbox-mcp setup [options]      Write MCP client config
  pgsandbox-mcp doctor [options]     Check config and Postgres connectivity
  pgsandbox-mcp local init [options] Initialize the managed local Postgres cluster
  pgsandbox-mcp local start [options] Start the managed local Postgres cluster
  pgsandbox-mcp local stop [options] Stop the managed local Postgres cluster
  pgsandbox-mcp local status [options] Show managed local Postgres status
  pgsandbox-mcp smoke-test [options] Create, query, and delete a sandbox

Setup options:
  --client <client>                  codex, cursor, vscode, claude-desktop, all
  --scope <scope>                    user or project
  --admin-url <url>                  Admin Postgres URL to write into config
  --postgres-version <major>          Managed local Postgres version, for example 16
  --command <command>                Command MCP clients should run
  --name <name>                      Server name in MCP config
  --dry-run                          Print config without writing
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_local_runtime_actions() {
        assert!(matches!(
            parse_local_command(&args(&["init"])).unwrap().action,
            LocalAction::Init
        ));
        assert!(matches!(
            parse_local_command(&args(&["start"])).unwrap().action,
            LocalAction::Start
        ));
        assert!(matches!(
            parse_local_command(&args(&["stop"])).unwrap().action,
            LocalAction::Stop
        ));
        assert!(matches!(
            parse_local_command(&args(&["status"])).unwrap().action,
            LocalAction::Status
        ));
    }

    #[test]
    fn parses_local_runtime_postgres_version_option() {
        let command = parse_local_command(&args(&["start", "--postgres-version", "17"])).unwrap();

        assert!(matches!(command.action, LocalAction::Start));
        assert_eq!(command.postgres_version.as_deref(), Some("17"));
    }

    #[test]
    fn help_text_lists_local_runtime_commands() {
        let help = help_text();

        assert!(help.contains("pgsandbox-mcp local init"));
        assert!(help.contains("pgsandbox-mcp local start"));
        assert!(help.contains("pgsandbox-mcp local stop"));
        assert!(help.contains("pgsandbox-mcp local status"));
    }
}
