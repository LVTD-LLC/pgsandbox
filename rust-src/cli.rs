use std::collections::BTreeMap;

use anyhow::Context;

use crate::{
    config::{load_config, load_config_from_env},
    doctor::run_doctor,
    mcp::serve_stdio,
    postgres::{CreateDatabaseInput, DatabaseSelector, PostgresSandboxManager, RunSqlInput},
    setup::{
        build_launch_config, config_snippet, parse_client, parse_scope, resolve_targets,
        write_client_config,
    },
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
        "smoke-test" if has_help_flag(&rest) => {
            print_help();
            Ok(0)
        }
        "setup" => setup(&rest).await,
        "doctor" => doctor(&rest).await,
        "smoke-test" => smoke_test(&rest).await,
        "" => start_server().await.map(|()| 0),
        other => anyhow::bail!("Unknown command: {other}"),
    }
}

async fn start_server() -> anyhow::Result<()> {
    serve_stdio(load_config()?).await
}

async fn setup(args: &[String]) -> anyhow::Result<u8> {
    let options = parse_options(args)?;
    let client = parse_client(options.get("client").map(String::as_str).unwrap_or("codex"))?;
    let scope = parse_scope(options.get("scope").map(String::as_str).unwrap_or("user"))?;
    let admin_url = options.get("admin-url").map(String::as_str);
    let launch = build_launch_config(
        options.get("name").map(String::as_str),
        options.get("command").map(String::as_str),
        admin_url,
    );
    let dry_run = options.contains_key("dry-run");
    let cwd = std::env::current_dir()?;
    let targets = resolve_targets(client, scope, &cwd)?;

    if admin_url.is_none() {
        eprintln!("No PGSANDBOX_ADMIN_DATABASE_URL was written. The MCP client must provide it in the server environment.");
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
    Ok(0)
}

async fn doctor(args: &[String]) -> anyhow::Result<u8> {
    let options = parse_options(args)?;
    let cwd = std::env::current_dir()?;
    let result = run_doctor(options.get("admin-url").map(String::as_str), &cwd).await;
    for line in result.lines {
        println!("{line}");
    }
    Ok(if result.ok { 0 } else { 1 })
}

async fn smoke_test(args: &[String]) -> anyhow::Result<u8> {
    let options = parse_options(args)?;
    let env = if let Some(admin_url) = options.get("admin-url") {
        let mut env = std::env::vars().collect::<Vec<_>>();
        env.push((
            "PGSANDBOX_ADMIN_DATABASE_URL".to_string(),
            admin_url.to_string(),
        ));
        Some(env)
    } else {
        None
    };
    let config = match env {
        Some(env) => load_config_from_env(env)?,
        None => load_config()?,
    };
    let manager = PostgresSandboxManager::new(config);
    let mut database_id = None;

    let result = async {
        let created = manager
            .create_database(CreateDatabaseInput {
                profile: None,
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
                database_id: database_id.clone(),
                database_name: None,
                sql: "update items set name = 'not returning' where id = 1".to_string(),
                readonly: Some(false),
                row_limit: None,
            })
            .await?;
        anyhow::ensure!(
            updated.row_count == Some(1) && updated.rows.is_empty(),
            "DML with 'returning' inside a string literal was not handled as a direct query"
        );
        println!("{}", serde_json::to_string_pretty(&updated)?);

        let query = manager
            .run_sql(RunSqlInput {
                profile: None,
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
                database_id: Some(database_id),
                database_name: None,
            })
            .await;
    }

    result?;
    Ok(0)
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

fn print_help() {
    println!(
        r#"pgsandbox-mcp {VERSION}

Usage:
  pgsandbox-mcp                      Start the MCP server over stdio
  pgsandbox-mcp stdio                Start the MCP server over stdio
  pgsandbox-mcp setup [options]      Write MCP client config
  pgsandbox-mcp doctor [options]     Check config and Postgres connectivity
  pgsandbox-mcp smoke-test [options] Create, query, and delete a sandbox

Setup options:
  --client <client>                  codex, cursor, vscode, claude-desktop, all
  --scope <scope>                    user or project
  --admin-url <url>                  Admin Postgres URL to write into config
  --command <command>                Command MCP clients should run
  --name <name>                      Server name in MCP config
  --dry-run                          Print config without writing
"#
    );
}
