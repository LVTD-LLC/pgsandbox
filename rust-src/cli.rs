use std::{
    collections::BTreeMap,
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

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
        "upgrade" if has_help_flag(&rest) => {
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
        "upgrade" => upgrade(&rest).await,
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

    if setup_should_prepare_managed_local(admin_url, dry_run) {
        ensure_setup_managed_local(options.get("postgres-version").map(String::as_str))?;
    } else if admin_url.is_none() {
        println!(
            "Dry run: managed local Postgres was not checked or started. Omit --dry-run to prepare it."
        );
    } else {
        println!("Using explicit admin URL; managed local Postgres setup was skipped.");
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

async fn upgrade(args: &[String]) -> anyhow::Result<u8> {
    let started = std::time::Instant::now();
    let telemetry = Telemetry::new(crate::config::load_telemetry_config());
    let options = parse_options(args)?;
    let dry_run = options.contains_key("dry-run");
    let current_exe = env::current_exe().context("failed to resolve current executable")?;
    let plan = detect_upgrade_plan(&current_exe);

    match plan.source {
        UpgradeInstallSource::Homebrew => {
            run_homebrew_upgrade(dry_run)?;
        }
        UpgradeInstallSource::DirectRelease => {
            run_github_installer_upgrade(&current_exe, &options, dry_run).await?;
        }
        UpgradeInstallSource::Cargo => {
            anyhow::bail!(
                "`pgsandbox-mcp upgrade` does not replace Cargo or source builds. Reinstall with `cargo install --git https://github.com/LVTD-LLC/pgsandbox-mcp --tag v{VERSION} --force`, or use Homebrew/the GitHub install script for managed upgrades."
            );
        }
    }

    if !options.contains_key("no-setup") {
        let client = parse_client(
            options
                .get("setup")
                .or_else(|| options.get("client"))
                .map(String::as_str)
                .unwrap_or("all"),
        )?;
        let scope = parse_scope(options.get("scope").map(String::as_str).unwrap_or("user"))?;
        let setup_args = upgrade_setup_args(&options, client, scope, &plan.setup_command);
        run_post_upgrade_command(&plan.runner_command, setup_args, dry_run)?;
    } else {
        println!("Skipped setup because --no-setup was provided.");
    }

    if !options.contains_key("no-doctor") {
        let doctor_args = upgrade_doctor_args(&options);
        run_post_upgrade_command(&plan.runner_command, doctor_args, dry_run)?;
    } else {
        println!("Skipped doctor because --no-doctor was provided.");
    }

    println!("Next: restart MCP clients so they launch the updated server.");
    telemetry
        .capture(
            crate::telemetry::EVENT_CLI_COMMAND_COMPLETED,
            properties([
                ("command", serde_json::json!("upgrade")),
                ("dryRun", serde_json::json!(dry_run)),
                (
                    "installSource",
                    serde_json::json!(plan.source.telemetry_name()),
                ),
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
            "--no-setup" => {
                options.insert("no-setup".to_string(), "true".to_string());
                index += 1;
            }
            "--no-doctor" => {
                options.insert("no-doctor".to_string(), "true".to_string());
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpgradeInstallSource {
    Homebrew,
    DirectRelease,
    Cargo,
}

impl UpgradeInstallSource {
    fn telemetry_name(self) -> &'static str {
        match self {
            UpgradeInstallSource::Homebrew => "homebrew",
            UpgradeInstallSource::DirectRelease => "direct-release",
            UpgradeInstallSource::Cargo => "cargo",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpgradePlan {
    source: UpgradeInstallSource,
    runner_command: PathBuf,
    setup_command: String,
}

fn detect_upgrade_plan(current_exe: &Path) -> UpgradePlan {
    if let Some(homebrew_bin) = homebrew_bin_path_if_managed(current_exe) {
        let command = if homebrew_bin.exists() {
            homebrew_bin
        } else {
            PathBuf::from("pgsandbox-mcp")
        };
        return UpgradePlan {
            source: UpgradeInstallSource::Homebrew,
            setup_command: command.to_string_lossy().to_string(),
            runner_command: command,
        };
    }

    if path_looks_like_cargo_or_source_build(current_exe) {
        return UpgradePlan {
            source: UpgradeInstallSource::Cargo,
            runner_command: current_exe.to_path_buf(),
            setup_command: current_exe.to_string_lossy().to_string(),
        };
    }

    UpgradePlan {
        source: UpgradeInstallSource::DirectRelease,
        runner_command: current_exe.to_path_buf(),
        setup_command: current_exe.to_string_lossy().to_string(),
    }
}

fn homebrew_bin_path_if_managed(current_exe: &Path) -> Option<PathBuf> {
    let current = canonical_or_original(current_exe);
    let formula_prefixes = ["LVTD-LLC/tap/pgsandbox-mcp", "pgsandbox-mcp"]
        .into_iter()
        .filter_map(homebrew_prefix_for);

    for prefix in formula_prefixes {
        let prefix = canonical_or_original(&prefix);
        if current.starts_with(&prefix) {
            return Some(homebrew_global_bin_path());
        }
    }

    path_has_homebrew_cellar_pgsandbox(&current).then(homebrew_global_bin_path)
}

fn homebrew_prefix_for(formula: &str) -> Option<PathBuf> {
    command_output_trim("brew", &["--prefix", formula]).map(PathBuf::from)
}

fn homebrew_global_bin_path() -> PathBuf {
    command_output_trim("brew", &["--prefix"])
        .map(|prefix| PathBuf::from(prefix).join("bin").join("pgsandbox-mcp"))
        .unwrap_or_else(|| PathBuf::from("pgsandbox-mcp"))
}

fn command_output_trim(command: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(command)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn path_has_homebrew_cellar_pgsandbox(path: &Path) -> bool {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    components
        .windows(2)
        .any(|window| window[0] == "Cellar" && window[1] == "pgsandbox-mcp")
}

fn path_looks_like_cargo_or_source_build(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/.cargo/bin/pgsandbox-mcp")
        || text.contains("\\.cargo\\bin\\pgsandbox-mcp")
        || text.contains("/target/debug/pgsandbox-mcp")
        || text.contains("/target/release/pgsandbox-mcp")
        || text.contains("\\target\\debug\\pgsandbox-mcp")
        || text.contains("\\target\\release\\pgsandbox-mcp")
}

fn run_homebrew_upgrade(dry_run: bool) -> anyhow::Result<()> {
    println!("Upgrade source: Homebrew");
    run_status_command("brew", &["update"], dry_run)?;
    run_status_command("brew", &["upgrade", "LVTD-LLC/tap/pgsandbox-mcp"], dry_run)
}

async fn run_github_installer_upgrade(
    current_exe: &Path,
    options: &BTreeMap<String, String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    ensure_github_installer_supported()?;
    let install_dir = current_exe.parent().with_context(|| {
        format!(
            "could not resolve install directory for {}",
            current_exe.display()
        )
    })?;
    let script_url = options
        .get("install-script-url")
        .cloned()
        .or_else(|| env::var("PGSANDBOX_INSTALL_SCRIPT_URL").ok())
        .unwrap_or_else(|| {
            "https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh"
                .to_string()
        });

    println!("Upgrade source: GitHub release installer");
    println!("Install dir: {}", install_dir.display());
    println!("Installer: {script_url}");

    if dry_run {
        println!("Dry run: installer was not downloaded or run.");
        return Ok(());
    }

    let script = reqwest::get(&script_url)
        .await
        .with_context(|| format!("failed to download installer from {script_url}"))?
        .error_for_status()
        .with_context(|| format!("installer download failed for {script_url}"))?
        .text()
        .await
        .context("failed to read installer response")?;
    let script_path = temp_upgrade_script_path();
    fs::write(&script_path, script)
        .with_context(|| format!("failed to write installer to {}", script_path.display()))?;

    let mut command = Command::new("sh");
    command
        .arg(&script_path)
        .env("PGSANDBOX_INSTALL_DIR", install_dir);
    apply_installer_env_overrides(&mut command, options);

    let status = command
        .status()
        .with_context(|| format!("failed to run installer with sh {}", script_path.display()));
    let _ = fs::remove_file(&script_path);
    let status = status?;

    if !status.success() {
        anyhow::bail!("GitHub release installer failed with {status}");
    }
    Ok(())
}

fn ensure_github_installer_supported() -> anyhow::Result<()> {
    match env::consts::OS {
        "macos" | "linux" => Ok(()),
        other => anyhow::bail!(
            "`pgsandbox-mcp upgrade` supports the GitHub release installer on macOS and Linux. {other} needs a platform-specific release installer before self-upgrade can be supported."
        ),
    }
}

fn temp_upgrade_script_path() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    env::temp_dir().join(format!(
        "pgsandbox-mcp-install-{}-{timestamp}.sh",
        std::process::id()
    ))
}

fn apply_installer_env_overrides(command: &mut Command, options: &BTreeMap<String, String>) {
    for (option, env_name) in [
        ("version", "PGSANDBOX_VERSION"),
        ("repo", "PGSANDBOX_REPO"),
        ("github-base-url", "PGSANDBOX_GITHUB_BASE_URL"),
        ("github-api-url", "PGSANDBOX_GITHUB_API_URL"),
        ("target", "PGSANDBOX_TARGET"),
        ("skip-checksum", "PGSANDBOX_SKIP_CHECKSUM"),
    ] {
        if let Some(value) = options.get(option) {
            command.env(env_name, value);
        }
    }
}

fn upgrade_setup_args(
    options: &BTreeMap<String, String>,
    client: crate::setup::ClientSelector,
    scope: crate::setup::ConfigScope,
    setup_command: &str,
) -> Vec<String> {
    let mut args = vec![
        "setup".to_string(),
        "--client".to_string(),
        client_selector_name(client).to_string(),
        "--scope".to_string(),
        scope.to_string(),
        "--command".to_string(),
        options
            .get("command")
            .cloned()
            .unwrap_or_else(|| setup_command.to_string()),
    ];

    for option in ["admin-url", "postgres-version", "name"] {
        if let Some(value) = options.get(option) {
            args.push(format!("--{option}"));
            args.push(value.clone());
        }
    }

    args
}

fn upgrade_doctor_args(options: &BTreeMap<String, String>) -> Vec<String> {
    let mut args = vec!["doctor".to_string()];
    for option in ["admin-url", "postgres-version"] {
        if let Some(value) = options.get(option) {
            args.push(format!("--{option}"));
            args.push(value.clone());
        }
    }
    args
}

fn run_post_upgrade_command(
    command: &Path,
    args: Vec<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let display_args = mask_command_args_for_display(&args);
    let display_args = display_args.iter().map(String::as_str).collect::<Vec<_>>();
    println!("Running: {}", command_display(command, &display_args));

    if dry_run {
        return Ok(());
    }

    let status = Command::new(command)
        .args(&args)
        .status()
        .with_context(|| format!("failed to run {}", command.display()))?;

    if !status.success() {
        anyhow::bail!("{} failed with {status}", command.display());
    }

    Ok(())
}

fn mask_command_args_for_display(args: &[String]) -> Vec<String> {
    let mut display = Vec::with_capacity(args.len());
    let mut mask_next = false;

    for arg in args {
        if mask_next {
            display.push(mask_connection_string(arg));
            mask_next = false;
            continue;
        }

        if let Some(value) = arg.strip_prefix("--admin-url=") {
            display.push(format!("--admin-url={}", mask_connection_string(value)));
            continue;
        }

        display.push(arg.clone());
        if arg == "--admin-url" {
            mask_next = true;
        }
    }

    display
}

fn run_status_command(command: &str, args: &[&str], dry_run: bool) -> anyhow::Result<()> {
    println!("Running: {}", shell_command_text(command, args));
    if dry_run {
        return Ok(());
    }

    let status = Command::new(command)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {}", shell_command_text(command, args)))?;

    if !status.success() {
        anyhow::bail!("{} failed with {status}", shell_command_text(command, args));
    }

    Ok(())
}

fn command_display(command: &Path, args: &[&str]) -> String {
    let command = command.to_string_lossy();
    shell_command_text(&command, args)
}

fn shell_command_text(command: &str, args: &[&str]) -> String {
    std::iter::once(command)
        .chain(args.iter().copied())
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if value.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || matches!(character, '/' | '.' | '-' | '_' | ':' | '=' | '@')
    }) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn setup_should_prepare_managed_local(admin_url: Option<&str>, dry_run: bool) -> bool {
    admin_url.is_none() && !dry_run
}

fn ensure_setup_managed_local(postgres_version: Option<&str>) -> anyhow::Result<()> {
    println!("Checking managed local Postgres runtime...");
    let cluster = LocalPostgresCluster::from_env_for_version(postgres_version)?;

    match cluster.ensure_started() {
        Ok(config) => {
            println!("Local Postgres: running");
            print_local_config(&config, &cluster);
            Ok(())
        }
        Err(error) if setup_error_is_missing_local_postgres(&error) => {
            install_local_postgres_with_homebrew(postgres_version)?;
            let config = cluster
                .ensure_started()
                .context("failed to start managed local Postgres after installing PostgreSQL")?;
            println!("Local Postgres: running");
            print_local_config(&config, &cluster);
            Ok(())
        }
        Err(error) => Err(error).context("failed to start managed local Postgres"),
    }
}

fn setup_error_is_missing_local_postgres(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string();
        message.starts_with("could not find local Postgres")
            || message.starts_with(
                "Postgres server binaries `initdb`, `pg_ctl`, and `postgres` were not found",
            )
    })
}

fn install_local_postgres_with_homebrew(postgres_version: Option<&str>) -> anyhow::Result<()> {
    let package = homebrew_postgres_package(postgres_version)?;
    ensure_homebrew_available()?;

    println!("Postgres server binaries were not found.");
    println!("Installing PostgreSQL with Homebrew: brew install {package}");

    let status = Command::new("brew")
        .arg("install")
        .arg(&package)
        .status()
        .with_context(|| format!("failed to run `brew install {package}`"))?;
    if !status.success() {
        anyhow::bail!("`brew install {package}` failed with {status}");
    }
    Ok(())
}

fn ensure_homebrew_available() -> anyhow::Result<()> {
    match Command::new("brew")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => anyhow::bail!(
            "Homebrew is installed, but `brew --version` failed with {status}"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => anyhow::bail!(
            "Postgres server binaries are missing and Homebrew is not available. Install Homebrew and rerun `pgsandbox-mcp setup`, or install PostgreSQL manually and set PGSANDBOX_POSTGRES_BIN_DIR."
        ),
        Err(error) => Err(error).context("failed to run `brew --version`"),
    }
}

fn homebrew_postgres_package(postgres_version: Option<&str>) -> anyhow::Result<String> {
    match postgres_version
        .map(normalize_setup_postgres_version)
        .transpose()?
    {
        Some(version) => Ok(format!("postgresql@{version}")),
        None => Ok("postgresql".to_string()),
    }
}

fn normalize_setup_postgres_version(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    let major = value
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    if major.is_empty() {
        anyhow::bail!("postgres-version must start with a numeric major version");
    }
    Ok(major)
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
  pgsandbox-mcp setup [options]      Check and start managed local Postgres, then write MCP client config
  pgsandbox-mcp doctor [options]     Check config and Postgres connectivity
  pgsandbox-mcp upgrade [options]    Upgrade the binary, then run setup all and doctor
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
  --dry-run                          Print config without writing or preparing local Postgres

Upgrade options:
  --setup <client>                   Setup client after upgrade; defaults to all
  --client <client>                  Alias for --setup
  --scope <scope>                    Setup scope; defaults to user
  --version <version>                Install a specific GitHub release version
  --no-setup                         Skip post-upgrade setup
  --no-doctor                        Skip post-upgrade doctor
  --dry-run                          Print upgrade actions without running them
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

    #[test]
    fn help_text_lists_upgrade_command() {
        let help = help_text();

        assert!(help.contains("pgsandbox-mcp upgrade [options]"));
        assert!(help.contains("--setup <client>"));
        assert!(help.contains("--no-setup"));
    }

    #[test]
    fn parse_options_accepts_upgrade_boolean_flags() {
        let options =
            parse_options(&args(&["--no-setup", "--no-doctor", "--setup", "codex"])).unwrap();

        assert_eq!(options.get("no-setup").map(String::as_str), Some("true"));
        assert_eq!(options.get("no-doctor").map(String::as_str), Some("true"));
        assert_eq!(options.get("setup").map(String::as_str), Some("codex"));
    }

    #[test]
    fn detects_cargo_or_source_upgrade_path() {
        let plan = detect_upgrade_plan(Path::new("/repo/target/debug/pgsandbox-mcp"));

        assert_eq!(plan.source, UpgradeInstallSource::Cargo);
    }

    #[test]
    fn detects_direct_release_upgrade_path() {
        let plan = detect_upgrade_plan(Path::new("/home/user/.local/bin/pgsandbox-mcp"));

        assert_eq!(plan.source, UpgradeInstallSource::DirectRelease);
    }

    #[test]
    fn detects_homebrew_cellar_path_shape() {
        assert!(path_has_homebrew_cellar_pgsandbox(Path::new(
            "/opt/homebrew/Cellar/pgsandbox-mcp/0.4.5/bin/pgsandbox-mcp"
        )));
    }

    #[test]
    fn upgrade_setup_args_default_to_all_user_with_absolute_command() {
        let options = BTreeMap::from([("postgres-version".to_string(), "18".to_string())]);
        let setup_args = upgrade_setup_args(
            &options,
            crate::setup::ClientSelector::All,
            crate::setup::ConfigScope::User,
            "/usr/local/bin/pgsandbox-mcp",
        );

        assert_eq!(
            setup_args,
            args(&[
                "setup",
                "--client",
                "all",
                "--scope",
                "user",
                "--command",
                "/usr/local/bin/pgsandbox-mcp",
                "--postgres-version",
                "18"
            ])
        );
    }

    #[test]
    fn upgrade_doctor_args_forward_runtime_options() {
        let options = BTreeMap::from([
            (
                "admin-url".to_string(),
                "postgres://admin:secret@localhost/postgres".to_string(),
            ),
            ("postgres-version".to_string(), "17".to_string()),
        ]);
        let doctor_args = upgrade_doctor_args(&options);

        assert_eq!(
            doctor_args,
            args(&[
                "doctor",
                "--admin-url",
                "postgres://admin:secret@localhost/postgres",
                "--postgres-version",
                "17"
            ])
        );
    }

    #[test]
    fn post_upgrade_command_display_masks_admin_url() {
        let display = mask_command_args_for_display(&args(&[
            "setup",
            "--admin-url",
            "postgres://admin:secret@localhost/postgres",
            "--postgres-version",
            "17",
        ]));

        assert_eq!(
            display,
            args(&[
                "setup",
                "--admin-url",
                "postgres://admin:****@localhost/postgres",
                "--postgres-version",
                "17",
            ])
        );
    }

    #[test]
    fn post_upgrade_command_display_masks_admin_url_equals_form() {
        let display = mask_command_args_for_display(&args(&[
            "doctor",
            "--admin-url=postgres://admin:secret@localhost/postgres",
        ]));

        assert_eq!(
            display,
            args(&[
                "doctor",
                "--admin-url=postgres://admin:****@localhost/postgres",
            ])
        );
    }

    #[test]
    fn setup_prepares_managed_local_runtime_by_default() {
        assert!(setup_should_prepare_managed_local(None, false));
    }

    #[test]
    fn setup_skips_managed_local_runtime_for_explicit_admin_url() {
        assert!(!setup_should_prepare_managed_local(
            Some("postgres://admin:secret@127.0.0.1/postgres"),
            false
        ));
    }

    #[test]
    fn setup_skips_managed_local_runtime_for_dry_run() {
        assert!(!setup_should_prepare_managed_local(None, true));
    }

    #[test]
    fn setup_installs_unversioned_homebrew_postgres_by_default() {
        assert_eq!(homebrew_postgres_package(None).unwrap(), "postgresql");
    }

    #[test]
    fn setup_installs_requested_homebrew_postgres_major_version() {
        assert_eq!(
            homebrew_postgres_package(Some("18.4")).unwrap(),
            "postgresql@18"
        );
    }

    #[test]
    fn setup_treats_runtime_missing_binary_message_as_installable() {
        let error = anyhow::anyhow!(
            "Postgres server binaries `initdb`, `pg_ctl`, and `postgres` were not found together on PATH or in common local install locations."
        );

        assert!(setup_error_is_missing_local_postgres(&error));
    }

    #[test]
    fn help_text_describes_setup_preflight() {
        let help = help_text();

        assert!(help.contains("Check and start managed local Postgres"));
    }
}
