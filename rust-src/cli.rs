use std::{
    collections::BTreeMap,
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    config::{load_config, load_config_deferred_local, load_config_from_env},
    doctor::{mask_connection_string, run_doctor},
    local::{
        local_postgres_install_plan, LocalClusterConfig, LocalClusterStatus, LocalPostgresCluster,
        LocalPostgresEnsureResult,
    },
    mcp::{serve_stdio, tool_result_payload},
    postgres::{
        CleanupExpiredInput, CloneDatabaseInput, ConnectionStringInput, CreateDatabaseInput,
        CreateSandboxFromTemplateInput, CreateSchemaSnapshotInput, CreateTemplateFromSandboxInput,
        DatabaseSelector, DeleteSchemaSnapshotInput, DeleteTemplateInput, DescribeSchemaInput,
        DiffSchemaSnapshotInput, ExplainQueryInput, ListDatabasesInput, ListExtensionsInput,
        ListProfilesInput, ListSchemaSnapshotsInput, ListTemplatesInput, PostgresSandboxManager,
        PrepareForRepoInput, RunRepoCommandInput, RunSqlInput, RunSqlOutput, SchemaDiffInput,
        SeedDatabaseInput, ValidateSchemaChangeInput,
    },
    setup::{
        build_launch_config, config_snippet, parse_client, parse_scope, resolve_targets,
        write_client_config,
    },
    telemetry::{properties, Telemetry},
    VERSION,
};

const UNINSTALL_SCRIPT: &str = include_str!("../scripts/uninstall.sh");

pub async fn run(args: Vec<String>) -> anyhow::Result<u8> {
    let (command, rest) = args
        .split_first()
        .map(|(command, rest)| (command.as_str(), rest.to_vec()))
        .unwrap_or(("mcp", Vec::new()));

    match command {
        "mcp" | "stdio" => start_server().await.map(|()| 0),
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
        "list-extensions" if has_help_flag(&rest) => {
            print_help();
            Ok(0)
        }
        "ensure-postgres" if has_help_flag(&rest) => {
            print_help();
            Ok(0)
        }
        "upgrade" if has_help_flag(&rest) => {
            print_help();
            Ok(0)
        }
        "uninstall" if has_help_flag(&rest) => uninstall(&rest).await,
        "local" if has_help_flag(&rest) => {
            print_help();
            Ok(0)
        }
        "smoke-test" if has_help_flag(&rest) => {
            print_help();
            Ok(0)
        }
        "setup" => setup(&rest).await,
        "tool" => cli_tool_from_tool_subcommand(&rest).await,
        "doctor" if uses_tool_json_mode(&rest) => cli_tool(command, &rest).await,
        "doctor" => doctor(&rest).await,
        "list-extensions" if uses_tool_json_mode(&rest) => cli_tool(command, &rest).await,
        "list-extensions" => list_extensions(&rest).await,
        "ensure-postgres" if uses_tool_json_mode(&rest) => cli_tool(command, &rest).await,
        "ensure-postgres" => ensure_postgres(&rest).await,
        "upgrade" => upgrade(&rest).await,
        "uninstall" => uninstall(&rest).await,
        "local" => local(&rest).await,
        "smoke-test" => smoke_test(&rest).await,
        "" => start_server().await.map(|()| 0),
        other if find_cli_tool_command(other).is_some() => cli_tool(other, &rest).await,
        other => anyhow::bail!("Unknown command: {other}"),
    }
}

async fn start_server() -> anyhow::Result<()> {
    serve_stdio(load_config_deferred_local()?).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CliToolCommand {
    command_name: &'static str,
    tool_name: &'static str,
}

const CLI_TOOL_COMMANDS: &[CliToolCommand] = &[
    CliToolCommand {
        command_name: "list-profiles",
        tool_name: "list_profiles",
    },
    CliToolCommand {
        command_name: "ensure-postgres",
        tool_name: "ensure_postgres",
    },
    CliToolCommand {
        command_name: "list-extensions",
        tool_name: "list_extensions",
    },
    CliToolCommand {
        command_name: "create-database",
        tool_name: "create_database",
    },
    CliToolCommand {
        command_name: "clone-database",
        tool_name: "clone_database",
    },
    CliToolCommand {
        command_name: "delete-database",
        tool_name: "delete_database",
    },
    CliToolCommand {
        command_name: "get-connection-string",
        tool_name: "get_connection_string",
    },
    CliToolCommand {
        command_name: "run-sql",
        tool_name: "run_sql",
    },
    CliToolCommand {
        command_name: "describe-schema",
        tool_name: "describe_schema",
    },
    CliToolCommand {
        command_name: "schema-digest",
        tool_name: "schema_digest",
    },
    CliToolCommand {
        command_name: "schema-diff",
        tool_name: "schema_diff",
    },
    CliToolCommand {
        command_name: "explain-query",
        tool_name: "explain_query",
    },
    CliToolCommand {
        command_name: "create-schema-snapshot",
        tool_name: "create_schema_snapshot",
    },
    CliToolCommand {
        command_name: "list-schema-snapshots",
        tool_name: "list_schema_snapshots",
    },
    CliToolCommand {
        command_name: "delete-schema-snapshot",
        tool_name: "delete_schema_snapshot",
    },
    CliToolCommand {
        command_name: "diff-schema-snapshot",
        tool_name: "diff_schema_snapshot",
    },
    CliToolCommand {
        command_name: "prepare-for-repo",
        tool_name: "prepare_for_repo",
    },
    CliToolCommand {
        command_name: "run-repo-command",
        tool_name: "run_repo_command",
    },
    CliToolCommand {
        command_name: "validate-schema-change",
        tool_name: "validate_schema_change",
    },
    CliToolCommand {
        command_name: "seed-database",
        tool_name: "seed_database",
    },
    CliToolCommand {
        command_name: "create-template-from-sandbox",
        tool_name: "create_template_from_sandbox",
    },
    CliToolCommand {
        command_name: "create-sandbox-from-template",
        tool_name: "create_sandbox_from_template",
    },
    CliToolCommand {
        command_name: "list-templates",
        tool_name: "list_templates",
    },
    CliToolCommand {
        command_name: "delete-template",
        tool_name: "delete_template",
    },
    CliToolCommand {
        command_name: "list-databases",
        tool_name: "list_databases",
    },
    CliToolCommand {
        command_name: "cleanup-expired",
        tool_name: "cleanup_expired",
    },
    CliToolCommand {
        command_name: "doctor",
        tool_name: "doctor",
    },
];

#[derive(Debug, Clone, PartialEq)]
struct CliToolInvocation {
    tool_name: &'static str,
    input: Value,
    admin_url: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct CliEnsurePostgresInput {
    postgres_version: Option<String>,
    install_missing: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CliEnsurePostgresOutput {
    server_version: String,
    profile_name: String,
    postgres_version: Option<String>,
    install_missing: bool,
    install_method: Option<String>,
    installed_package: Option<String>,
    port: u16,
    data_dir: PathBuf,
    socket_dir: PathBuf,
    config_path: PathBuf,
    admin_url_redacted: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct CliDoctorInput {
    postgres_version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CliDoctorOutput {
    ok: bool,
    server_version: String,
    tool_count: usize,
    available_postgres_versions: Vec<String>,
    lines: Vec<String>,
}

async fn cli_tool_from_tool_subcommand(args: &[String]) -> anyhow::Result<u8> {
    let (tool, rest) = args
        .split_first()
        .map(|(tool, rest)| (tool.as_str(), rest))
        .context("Missing tool name. Use `pgsandbox tool <tool-name> --input '{...}'`.")?;
    cli_tool(tool, rest).await
}

async fn cli_tool(command: &str, args: &[String]) -> anyhow::Result<u8> {
    if has_help_flag(args) {
        print_tool_help(command);
        return Ok(0);
    }

    let invocation = parse_cli_tool_invocation(command, args)?;
    let (ok, payload) = execute_cli_tool(invocation).await?;
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(if ok { 0 } else { 1 })
}

async fn execute_cli_tool(invocation: CliToolInvocation) -> anyhow::Result<(bool, Value)> {
    match invocation.tool_name {
        "ensure_postgres" => {
            let input = cli_input::<CliEnsurePostgresInput>(invocation.input)?;
            tool_result_payload(
                async move {
                    let install_missing = input.install_missing.unwrap_or(true);
                    let cluster = LocalPostgresCluster::from_env_for_version(
                        input.postgres_version.as_deref(),
                    )?;
                    let result = cluster
                        .ensure_started_with_optional_install(install_missing)
                        .with_context(|| match input.postgres_version.as_deref() {
                            Some(version) => format!("failed to ensure local Postgres {version}"),
                            None => "failed to ensure default local Postgres".to_string(),
                        })?;
                    anyhow::Ok(CliEnsurePostgresOutput {
                        server_version: VERSION.to_string(),
                        profile_name: result.config.profile_name,
                        postgres_version: result.config.postgres_version,
                        install_missing,
                        install_method: result.install_method,
                        installed_package: result.installed_package,
                        port: result.config.port,
                        data_dir: result.config.data_dir,
                        socket_dir: result.config.socket_dir,
                        config_path: cluster.config_path(),
                        admin_url_redacted: mask_connection_string(&result.config.admin_url),
                    })
                }
                .await,
            )
        }
        "doctor" => {
            let input = cli_input::<CliDoctorInput>(invocation.input)?;
            let admin_url = invocation.admin_url;
            tool_result_payload(
                async move {
                    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    let result = run_doctor(
                        admin_url.as_deref(),
                        input.postgres_version.as_deref(),
                        &cwd,
                    )
                    .await;
                    anyhow::Ok(CliDoctorOutput {
                        ok: result.ok,
                        server_version: VERSION.to_string(),
                        tool_count: crate::mcp::PUBLIC_MCP_TOOL_COUNT,
                        available_postgres_versions: result.available_postgres_versions,
                        lines: result.lines,
                    })
                }
                .await,
            )
        }
        tool_name => {
            let config = cli_tool_config(invocation.admin_url.as_deref())?;
            let manager = PostgresSandboxManager::new(config);
            match tool_name {
                "list_profiles" => {
                    cli_manager_result(
                        manager.list_profiles(cli_input::<ListProfilesInput>(invocation.input)?),
                    )
                }
                "list_extensions" => cli_manager_result(
                    manager
                        .list_extensions(cli_input::<ListExtensionsInput>(invocation.input)?)
                        .await,
                ),
                "create_database" => cli_manager_result(
                    manager
                        .create_database(cli_input::<CreateDatabaseInput>(invocation.input)?)
                        .await,
                ),
                "clone_database" => cli_manager_result(
                    manager
                        .clone_database(cli_input::<CloneDatabaseInput>(invocation.input)?)
                        .await,
                ),
                "delete_database" => cli_manager_result(
                    manager
                        .delete_database(cli_input::<DatabaseSelector>(invocation.input)?)
                        .await,
                ),
                "get_connection_string" => {
                    let input = cli_input::<ConnectionStringInput>(invocation.input)?;
                    let include_credentials = input.include_credentials.unwrap_or(false);
                    cli_manager_result(
                        manager
                            .get_connection_string(DatabaseSelector::from(&input))
                            .await
                            .map(|output| output.with_credentials_in_response(include_credentials)),
                    )
                }
                "run_sql" => cli_manager_result(
                    manager
                        .run_sql(cli_input::<RunSqlInput>(invocation.input)?)
                        .await,
                ),
                "describe_schema" => cli_manager_result(
                    manager
                        .describe_schema(cli_input::<DescribeSchemaInput>(invocation.input)?)
                        .await,
                ),
                "schema_digest" => cli_manager_result(
                    manager
                        .schema_digest(cli_input::<DatabaseSelector>(invocation.input)?)
                        .await,
                ),
                "schema_diff" => cli_manager_result(
                    manager
                        .schema_diff(cli_input::<SchemaDiffInput>(invocation.input)?)
                        .await,
                ),
                "explain_query" => cli_manager_result(
                    manager
                        .explain_query(cli_input::<ExplainQueryInput>(invocation.input)?)
                        .await,
                ),
                "create_schema_snapshot" => cli_manager_result(
                    manager
                        .create_schema_snapshot(cli_input::<CreateSchemaSnapshotInput>(
                            invocation.input,
                        )?)
                        .await,
                ),
                "list_schema_snapshots" => cli_manager_result(
                    manager
                        .list_schema_snapshots(cli_input::<ListSchemaSnapshotsInput>(
                            invocation.input,
                        )?)
                        .await,
                ),
                "delete_schema_snapshot" => cli_manager_result(
                    manager
                        .delete_schema_snapshot(cli_input::<DeleteSchemaSnapshotInput>(
                            invocation.input,
                        )?)
                        .await,
                ),
                "diff_schema_snapshot" => cli_manager_result(
                    manager
                        .diff_schema_snapshot(cli_input::<DiffSchemaSnapshotInput>(
                            invocation.input,
                        )?)
                        .await,
                ),
                "prepare_for_repo" => cli_manager_result(
                    manager
                        .prepare_for_repo(cli_input::<PrepareForRepoInput>(invocation.input)?)
                        .await,
                ),
                "run_repo_command" => cli_manager_result(
                    manager
                        .run_repo_command(cli_input::<RunRepoCommandInput>(invocation.input)?)
                        .await,
                ),
                "validate_schema_change" => cli_manager_result(
                    manager
                        .validate_schema_change(cli_input::<ValidateSchemaChangeInput>(
                            invocation.input,
                        )?)
                        .await,
                ),
                "seed_database" => cli_manager_result(
                    manager
                        .seed_database(cli_input::<SeedDatabaseInput>(invocation.input)?)
                        .await,
                ),
                "create_template_from_sandbox" => cli_manager_result(
                    manager
                        .create_template_from_sandbox(cli_input::<CreateTemplateFromSandboxInput>(
                            invocation.input,
                        )?)
                        .await,
                ),
                "create_sandbox_from_template" => cli_manager_result(
                    manager
                        .create_sandbox_from_template(cli_input::<CreateSandboxFromTemplateInput>(
                            invocation.input,
                        )?)
                        .await,
                ),
                "list_templates" => cli_manager_result(
                    manager
                        .list_templates(cli_input::<ListTemplatesInput>(invocation.input)?)
                        .await,
                ),
                "delete_template" => cli_manager_result(
                    manager
                        .delete_template(cli_input::<DeleteTemplateInput>(invocation.input)?)
                        .await,
                ),
                "list_databases" => cli_manager_result(
                    manager
                        .list_databases(cli_input::<ListDatabasesInput>(invocation.input)?)
                        .await,
                ),
                "cleanup_expired" => cli_manager_result(
                    manager
                        .cleanup_expired(cli_input::<CleanupExpiredInput>(invocation.input)?)
                        .await,
                ),
                other => anyhow::bail!("Unsupported CLI tool command: {other}"),
            }
        }
    }
}

fn cli_manager_result<T: Serialize>(result: anyhow::Result<T>) -> anyhow::Result<(bool, Value)> {
    tool_result_payload(result)
}

fn cli_input<T>(input: Value) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(input).context("failed to parse CLI tool input")
}

fn cli_tool_config(admin_url: Option<&str>) -> anyhow::Result<crate::config::SandboxConfig> {
    if let Some(admin_url) = admin_url {
        let mut env = std::env::vars().collect::<Vec<_>>();
        env.push((
            "PGSANDBOX_ADMIN_DATABASE_URL".to_string(),
            admin_url.to_string(),
        ));
        return Ok(load_config_from_env(env)?);
    }
    Ok(load_config_deferred_local()?)
}

fn parse_cli_tool_invocation(command: &str, args: &[String]) -> anyhow::Result<CliToolInvocation> {
    let command = find_cli_tool_command(command)
        .with_context(|| format!("Unknown PGSandbox tool command: {command}"))?;
    let mut input = Map::new();
    let mut admin_url = None;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--json" => {
                index += 1;
            }
            "--input" => {
                let raw = next_value(args, index + 1, arg)?;
                merge_input_object(&mut input, parse_json_object(raw)?)?;
                index += 2;
            }
            "--input-file" => {
                let path = next_value(args, index + 1, arg)?;
                let raw = read_input_file(path)?;
                merge_input_object(&mut input, parse_json_object(&raw)?)?;
                index += 2;
            }
            "--admin-url" => {
                admin_url = Some(next_value(args, index + 1, arg)?.to_string());
                index += 2;
            }
            "--readonly"
            | "--include-credentials"
            | "--include-all-versions"
            | "--dry-run"
            | "--schema-only"
            | "--include-discovered-local"
            | "--install-missing" => {
                set_input_field(&mut input, flag_to_field(arg), Value::Bool(true));
                index += 1;
            }
            "--no-install" => {
                set_input_field(&mut input, "installMissing", Value::Bool(false));
                index += 1;
            }
            "--label" => {
                let raw = next_value(args, index + 1, arg)?;
                add_label(&mut input, raw)?;
                index += 2;
            }
            "--extension" => {
                push_array_field(
                    &mut input,
                    "extensions",
                    Value::String(next_value(args, index + 1, arg)?.to_string()),
                )?;
                index += 2;
            }
            "--postgres-version" => {
                set_input_field(
                    &mut input,
                    "postgresVersion",
                    Value::String(next_value(args, index + 1, arg)?.to_string()),
                );
                index += 2;
            }
            "--exclude-source-extension" => {
                push_array_field(
                    &mut input,
                    "excludeSourceExtensions",
                    Value::String(next_value(args, index + 1, arg)?.to_string()),
                )?;
                index += 2;
            }
            "--command" | "--migration-command" | "--seed-command" => {
                let field = flag_to_field(arg);
                let value = next_value(args, index + 1, arg)?;
                add_command_field(&mut input, field, value)?;
                index += 2;
            }
            "--set" => {
                let raw = next_value(args, index + 1, arg)?;
                let (field, value) = raw
                    .split_once('=')
                    .with_context(|| format!("{arg} expects field=value"))?;
                set_input_field(
                    &mut input,
                    option_name_to_field(field),
                    parse_scalar_value(value),
                );
                index += 2;
            }
            "--set-json" => {
                let raw = next_value(args, index + 1, arg)?;
                let (field, value) = raw
                    .split_once('=')
                    .with_context(|| format!("{arg} expects field=json"))?;
                let value = serde_json::from_str(value)
                    .with_context(|| format!("failed to parse JSON value for {field}"))?;
                set_input_field(&mut input, option_name_to_field(field), value);
                index += 2;
            }
            "--" => {
                let command = args[index + 1..]
                    .iter()
                    .map(|part| Value::String(part.clone()))
                    .collect::<Vec<_>>();
                set_input_field(&mut input, "command", Value::Array(command));
                index = args.len();
            }
            _ if arg.starts_with("--") => {
                let raw = &arg[2..];
                if let Some((name, value)) = raw.split_once('=') {
                    set_input_field(
                        &mut input,
                        option_name_to_field(name),
                        if name == "postgres-version" {
                            Value::String(value.to_string())
                        } else {
                            parse_scalar_value(value)
                        },
                    );
                    index += 1;
                } else {
                    let value = next_value(args, index + 1, arg)?;
                    set_input_field(
                        &mut input,
                        option_name_to_field(raw),
                        parse_scalar_value(value),
                    );
                    index += 2;
                }
            }
            _ => anyhow::bail!("Unexpected argument for {}: {arg}", command.command_name),
        }
    }

    Ok(CliToolInvocation {
        tool_name: command.tool_name,
        input: Value::Object(input),
        admin_url,
    })
}

fn find_cli_tool_command(command: &str) -> Option<&'static CliToolCommand> {
    CLI_TOOL_COMMANDS
        .iter()
        .find(|candidate| candidate.command_name == command || candidate.tool_name == command)
}

fn uses_tool_json_mode(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--json" | "--input" | "--input-file" | "--set" | "--set-json"
        )
    })
}

fn merge_input_object(target: &mut Map<String, Value>, value: Value) -> anyhow::Result<()> {
    let Value::Object(object) = value else {
        anyhow::bail!("CLI tool input must be a JSON object");
    };
    for (key, value) in object {
        target.insert(key, value);
    }
    Ok(())
}

fn parse_json_object(raw: &str) -> anyhow::Result<Value> {
    serde_json::from_str(raw).context("failed to parse JSON input")
}

fn read_input_file(path: &str) -> anyhow::Result<String> {
    if path == "-" {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .context("failed to read JSON input from stdin")?;
        return Ok(input);
    }
    fs::read_to_string(path).with_context(|| format!("failed to read JSON input file {path}"))
}

fn flag_to_field(flag: &str) -> &'static str {
    match flag {
        "--readonly" => "readonly",
        "--include-credentials" => "includeCredentials",
        "--include-all-versions" => "includeAllVersions",
        "--dry-run" => "dryRun",
        "--schema-only" => "schemaOnly",
        "--include-discovered-local" => "includeDiscoveredLocal",
        "--install-missing" => "installMissing",
        "--command" => "command",
        "--migration-command" => "migrationCommand",
        "--seed-command" => "seedCommand",
        _ => unreachable!("unsupported flag {flag}"),
    }
}

fn option_name_to_field(name: &str) -> String {
    let mut field = String::new();
    let mut uppercase_next = false;

    for character in name.chars() {
        if character == '-' || character == '_' {
            uppercase_next = true;
            continue;
        }

        if uppercase_next {
            field.extend(character.to_uppercase());
            uppercase_next = false;
        } else {
            field.push(character);
        }
    }

    field
}

fn set_input_field(target: &mut Map<String, Value>, field: impl Into<String>, value: Value) {
    target.insert(field.into(), value);
}

fn push_array_field(
    target: &mut Map<String, Value>,
    field: &'static str,
    value: Value,
) -> anyhow::Result<()> {
    match target
        .entry(field.to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        Value::Array(values) => {
            values.push(value);
            Ok(())
        }
        _ => anyhow::bail!("{field} must be an array"),
    }
}

fn add_label(target: &mut Map<String, Value>, raw: &str) -> anyhow::Result<()> {
    let (key, value) = raw
        .split_once('=')
        .with_context(|| "--label expects key=value")?;
    match target
        .entry("labels".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
    {
        Value::Object(labels) => {
            labels.insert(key.to_string(), parse_scalar_value(value));
            Ok(())
        }
        _ => anyhow::bail!("labels must be an object"),
    }
}

fn add_command_field(
    target: &mut Map<String, Value>,
    field: &'static str,
    value: &str,
) -> anyhow::Result<()> {
    if value.trim_start().starts_with('[') {
        let parsed = serde_json::from_str::<Vec<String>>(value)
            .with_context(|| format!("{field} JSON value must be an array of strings"))?;
        set_input_field(
            target,
            field,
            Value::Array(parsed.into_iter().map(Value::String).collect()),
        );
        return Ok(());
    }

    push_array_field(target, field, Value::String(value.to_string()))
}

fn parse_scalar_value(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
}

fn print_tool_help(command: &str) {
    let Some(command) = find_cli_tool_command(command) else {
        print_help();
        return;
    };
    println!(
        "Usage:\n  pgsandbox {name} --input '{{...}}'\n  pgsandbox {name} --input-file input.json\n  pgsandbox tool {tool} --input '{{...}}'\n\nCommon flags:\n  --profile <profile>\n  --postgres-version <major>\n  --database-id <id>\n  --database-name <name>\n  --json\n  --set field=value\n  --set-json field=json\n  --label key=value\n  --command <arg> [--command <arg>...]\n  -- --command-arg ...",
        name = command.command_name,
        tool = command.tool_name,
    );
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

    println!("Next: restart the MCP client, then run `pgsandbox doctor`.");
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

async fn list_extensions(args: &[String]) -> anyhow::Result<u8> {
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
    let input = ListExtensionsInput {
        profile: options.get("profile").cloned(),
        postgres_version: options.get("postgres-version").cloned(),
        database_id: options.get("database-id").cloned(),
        database_name: options.get("database-name").cloned(),
    };
    let has_database_selector = input.database_id.is_some() || input.database_name.is_some();

    let result = manager.list_extensions(input).await;
    match result {
        Ok(output) => println!("{}", serde_json::to_string_pretty(&output)?),
        Err(error) => {
            telemetry
                .capture(
                    crate::telemetry::EVENT_CLI_COMMAND_COMPLETED,
                    properties([
                        ("command", serde_json::json!("list-extensions")),
                        (
                            "hasProfile",
                            serde_json::json!(options.contains_key("profile")),
                        ),
                        (
                            "hasPostgresVersion",
                            serde_json::json!(options.contains_key("postgres-version")),
                        ),
                        (
                            "hasDatabaseSelector",
                            serde_json::json!(has_database_selector),
                        ),
                        ("success", serde_json::json!(false)),
                        (
                            "elapsedMs",
                            serde_json::json!(started.elapsed().as_millis()),
                        ),
                    ]),
                )
                .await;
            return Err(error);
        }
    }

    telemetry
        .capture(
            crate::telemetry::EVENT_CLI_COMMAND_COMPLETED,
            properties([
                ("command", serde_json::json!("list-extensions")),
                (
                    "hasProfile",
                    serde_json::json!(options.contains_key("profile")),
                ),
                (
                    "hasPostgresVersion",
                    serde_json::json!(options.contains_key("postgres-version")),
                ),
                (
                    "hasDatabaseSelector",
                    serde_json::json!(has_database_selector),
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

async fn ensure_postgres(args: &[String]) -> anyhow::Result<u8> {
    let started = std::time::Instant::now();
    let telemetry = Telemetry::new(crate::config::load_telemetry_config());
    let options = parse_options(args)?;
    let postgres_version = options.get("postgres-version").map(String::as_str);
    let install_missing = !options.contains_key("no-install");
    let dry_run = options.contains_key("dry-run");

    if dry_run {
        print_ensure_postgres_dry_run(postgres_version, install_missing)?;
    } else {
        let cluster = LocalPostgresCluster::from_env_for_version(postgres_version)?;
        let result = cluster
            .ensure_started_with_optional_install(install_missing)
            .context("failed to ensure managed local Postgres")?;
        print_local_ensure_result(&result, &cluster);
    }

    telemetry
        .capture(
            crate::telemetry::EVENT_CLI_COMMAND_COMPLETED,
            properties([
                ("command", serde_json::json!("ensure-postgres")),
                (
                    "hasPostgresVersion",
                    serde_json::json!(postgres_version.is_some()),
                ),
                ("installMissing", serde_json::json!(install_missing)),
                ("dryRun", serde_json::json!(dry_run)),
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

async fn upgrade(args: &[String]) -> anyhow::Result<u8> {
    let started = std::time::Instant::now();
    let telemetry = Telemetry::new(crate::config::load_telemetry_config());
    let options = parse_options(args)?;
    let dry_run = options.contains_key("dry-run");
    let current_exe = env::current_exe().context("failed to resolve current executable")?;
    let plan = detect_upgrade_plan(&current_exe);

    match plan.source {
        UpgradeInstallSource::Homebrew => {
            ensure_homebrew_upgrade_options(&options)?;
            run_homebrew_upgrade(dry_run)?;
        }
        UpgradeInstallSource::DirectRelease => {
            run_github_installer_upgrade(&current_exe, &options, dry_run).await?;
        }
        UpgradeInstallSource::Cargo => {
            anyhow::bail!(
                "`pgsandbox upgrade` does not replace Cargo or source builds. Reinstall with `cargo install --git https://github.com/LVTD-LLC/pgsandbox --tag v{VERSION} --force`, or use Homebrew/the GitHub install script for managed upgrades."
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

async fn uninstall(args: &[String]) -> anyhow::Result<u8> {
    let script_path = temp_uninstall_script_path();
    fs::write(&script_path, UNINSTALL_SCRIPT).with_context(|| {
        format!(
            "failed to write uninstall helper to {}",
            script_path.display()
        )
    })?;

    let current_exe = env::current_exe().context("failed to resolve current executable")?;
    let status = Command::new("sh")
        .arg(&script_path)
        .args(args)
        .env("PGSANDBOX_CURRENT_EXE", &current_exe)
        .env("PGSANDBOX_UNINSTALL_COMMAND_NAME", "pgsandbox uninstall")
        .status()
        .with_context(|| format!("failed to run uninstall helper {}", script_path.display()));
    let _ = fs::remove_file(&script_path);
    let status = status?;

    if !status.success() {
        anyhow::bail!("uninstall helper failed with {status}");
    }

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
            if command.install_missing {
                let result = cluster.ensure_started_with_optional_install(true)?;
                print_local_ensure_result(&result, &cluster);
            } else {
                let config = cluster.start()?;
                println!("Local Postgres: running");
                print_local_config(&config, &cluster);
            }
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
    let verbose = options.contains_key("verbose");
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
                extensions: None,
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
        println!(
            "{}",
            render_smoke_test_result("insert serialization", &inserted, verbose)?
        );

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
        println!(
            "{}",
            render_smoke_test_result(
                "trailing-comment INSERT RETURNING",
                &inserted_with_comment,
                verbose,
            )?
        );

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
        println!(
            "{}",
            render_smoke_test_result("non-returning update", &updated, verbose)?
        );

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
        println!(
            "{}",
            render_smoke_test_result("typed SELECT serialization", &query, verbose)?
        );

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
        println!(
            "{}",
            render_smoke_test_result("readonly literal guard", &readonly_literal, verbose)?
        );

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
    let elapsed_ms = started.elapsed().as_millis();
    println!(
        "Smoke test: {} ({elapsed_ms}ms)",
        if success { "PASS" } else { "FAIL" }
    );
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
                ("elapsedMs", serde_json::json!(elapsed_ms)),
            ]),
        )
        .await;
    result?;
    Ok(0)
}

fn render_smoke_test_result(
    label: &str,
    output: &RunSqlOutput,
    verbose: bool,
) -> anyhow::Result<String> {
    if verbose {
        return Ok(format!(
            "PASS {label}\n{}",
            serde_json::to_string_pretty(output)?
        ));
    }

    let affected = output
        .affected_row_count
        .map(|count| count.to_string())
        .unwrap_or_else(|| "n/a".to_string());
    Ok(format!(
        "PASS {label} (rows={}, affected={affected}, {}ms)",
        output.returned_row_count, output.elapsed_ms
    ))
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
    install_missing: bool,
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
        install_missing: options.contains_key("install-missing"),
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
            "--install-missing" => {
                options.insert("install-missing".to_string(), "true".to_string());
                index += 1;
            }
            "--no-install" => {
                options.insert("no-install".to_string(), "true".to_string());
                index += 1;
            }
            "--verbose" => {
                options.insert("verbose".to_string(), "true".to_string());
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
            PathBuf::from("pgsandbox")
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
    let formula_prefixes = ["LVTD-LLC/tap/pgsandbox", "pgsandbox"]
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
        .map(|prefix| PathBuf::from(prefix).join("bin").join("pgsandbox"))
        .unwrap_or_else(|| PathBuf::from("pgsandbox"))
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
        .any(|window| window[0] == "Cellar" && window[1] == "pgsandbox")
}

fn path_looks_like_cargo_or_source_build(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/.cargo/bin/pgsandbox")
        || text.contains("\\.cargo\\bin\\pgsandbox")
        || text.contains("/target/debug/pgsandbox")
        || text.contains("/target/release/pgsandbox")
        || text.contains("\\target\\debug\\pgsandbox")
        || text.contains("\\target\\release\\pgsandbox")
}

fn run_homebrew_upgrade(dry_run: bool) -> anyhow::Result<()> {
    println!("Upgrade source: Homebrew");
    run_status_command("brew", &["update"], dry_run)?;
    run_status_command("brew", &["upgrade", "LVTD-LLC/tap/pgsandbox"], dry_run)
}

fn ensure_homebrew_upgrade_options(options: &BTreeMap<String, String>) -> anyhow::Result<()> {
    if options.contains_key("version") {
        anyhow::bail!(
            "--version is only supported for GitHub install-script upgrades. Homebrew upgrades use the tap formula; omit --version or reinstall a pinned release with the GitHub installer."
        );
    }

    Ok(())
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
            "https://raw.githubusercontent.com/LVTD-LLC/pgsandbox/main/scripts/install.sh"
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
            "`pgsandbox upgrade` supports the GitHub release installer on macOS and Linux. {other} needs a platform-specific release installer before self-upgrade can be supported."
        ),
    }
}

fn temp_upgrade_script_path() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    env::temp_dir().join(format!(
        "pgsandbox-install-{}-{timestamp}.sh",
        std::process::id()
    ))
}

fn temp_uninstall_script_path() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    env::temp_dir().join(format!(
        "pgsandbox-uninstall-{}-{timestamp}.sh",
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
    let result = cluster
        .ensure_started_with_optional_install(true)
        .context("failed to start managed local Postgres")?;
    print_local_ensure_result(&result, &cluster);
    Ok(())
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
        println!("Next: pgsandbox local start");
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

fn print_local_ensure_result(result: &LocalPostgresEnsureResult, cluster: &LocalPostgresCluster) {
    if let (Some(method), Some(package)) = (&result.install_method, &result.installed_package) {
        println!("Installed PostgreSQL with {method}: {package}");
    }
    println!("Local Postgres: running");
    print_local_config(&result.config, cluster);
}

fn print_ensure_postgres_dry_run(
    postgres_version: Option<&str>,
    install_missing: bool,
) -> anyhow::Result<()> {
    let cluster = LocalPostgresCluster::from_env_for_version(postgres_version)?;
    println!("Dry run: managed local Postgres would be checked and started.");
    println!("Root: {}", cluster.root().display());
    if install_missing {
        if cluster.matching_binaries_available()? {
            println!("Matching Postgres binaries are already available; no package install would be needed.");
        } else {
            let plan = local_postgres_install_plan(postgres_version)?;
            println!(
                "If binaries are missing, PGSandbox would run: {}",
                plan.display_command
            );
        }
    } else {
        println!(
            "Missing Postgres binaries would not be installed because --no-install was provided."
        );
    }
    Ok(())
}

fn print_help() {
    println!("{}", help_text());
}

fn help_text() -> String {
    format!(
        r#"pgsandbox {VERSION}

Usage:
  pgsandbox                          Start the MCP server over stdio
  pgsandbox mcp                      Start the MCP server over stdio
  pgsandbox setup [options]          Check and start managed local Postgres, then write MCP client config
  pgsandbox doctor [options]         Check config and Postgres connectivity
  pgsandbox doctor --json            Return doctor output as the tool JSON envelope
  pgsandbox ensure-postgres          Install missing Postgres binaries when possible, then start managed local Postgres
  pgsandbox list-extensions [options] List available profile extensions and installed sandbox extensions
  pgsandbox local init [options]     Initialize the managed local Postgres cluster
  pgsandbox local start [options]    Start the managed local Postgres cluster
  pgsandbox local stop [options]     Stop the managed local Postgres cluster
  pgsandbox local status [options]   Show managed local Postgres status
  pgsandbox smoke-test [options]     Create, query, and delete a sandbox
  pgsandbox upgrade [options]        Upgrade the binary, then run setup all and doctor
  pgsandbox uninstall [options]      Remove PGSandbox binaries, MCP config entries, and local state

Sandbox CLI tool commands:
  pgsandbox create-database --name-hint <text> --ttl-minutes <minutes>
  pgsandbox run-sql --database-id <id> --sql <sql> [--readonly]
  pgsandbox delete-database --database-id <id>
  pgsandbox tool <mcp_tool_name> --input '{{...}}'
  pgsandbox <tool-command> --input-file input.json

Setup options:
  --client <client>                  codex, cursor, vscode, claude-desktop, all
  --scope <scope>                    user or project
  --admin-url <url>                  Admin Postgres URL to write into config
  --postgres-version <major>          Managed local Postgres version, for example 16
  --command <command>                Command MCP clients should run
  --name <name>                      Server name in MCP config
  --dry-run                          Print config without writing or preparing local Postgres

Postgres ensure options:
  --postgres-version <major>          Managed local Postgres version, for example 13
  --no-install                        Do not install missing Postgres binaries
  --dry-run                           Print what would be checked or installed

Smoke-test options:
  --verbose                           Print full structured SQL result dictionaries

Extension discovery options:
  --profile <profile>                 Target a configured profile
  --postgres-version <major>          Target a managed local Postgres version
  --database-id <id>                  Include installed extensions for a sandbox id
  --database-name <name>              Include installed extensions for a sandbox name
  --admin-url <url>                   Temporary admin Postgres URL for this command

Tool JSON options:
  --input <json>                      Merge a camelCase JSON object into the tool input
  --input-file <path|- >              Read a camelCase JSON object from a file or stdin
  --json                              Use the structured tool JSON envelope for commands with human output
  --set field=value                   Set any tool input field; values parse as JSON when possible
  --set-json field=json               Set any tool input field to a JSON value
  --label key=value                   Add a label filter or create label
  --command <arg>                     Add one argv part; repeat or pass a JSON array
  -- --command-arg ...                Set command to the remaining argv

Local options:
  --postgres-version <major>          Managed local Postgres version, for example 13
  --install-missing                   For local start, install missing Postgres binaries with a supported package manager

Upgrade options:
  --setup <client>                   Setup client after upgrade; defaults to all
  --client <client>                  Alias for --setup
  --scope <scope>                    Setup scope; defaults to user
  --version <version>                Install a specific GitHub release version
  --no-setup                         Skip post-upgrade setup
  --no-doctor                        Skip post-upgrade doctor
  --dry-run                          Print upgrade actions without running them

Uninstall options:
  --yes                              Do not prompt before deleting files or uninstalling packages
  --dry-run                          Print uninstall actions without changing files
  --server-name <name>               MCP server entry to remove from client configs
  --keep-binaries                    Do not uninstall packages or remove binaries
  --keep-client-config               Do not remove MCP client config entries
  --keep-state                       Do not remove ~/.pgsandbox or socket state
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

        assert!(help.contains("pgsandbox local init"));
        assert!(help.contains("pgsandbox local start"));
        assert!(help.contains("pgsandbox local stop"));
        assert!(help.contains("pgsandbox local status"));
    }

    #[test]
    fn help_text_presents_pgsandbox_as_cli_with_mcp_mode() {
        let help = help_text();

        assert!(help.starts_with("pgsandbox "));
        assert!(help.contains("pgsandbox mcp"));
        assert!(help.contains("pgsandbox create-database"));
        assert!(help.contains("pgsandbox run-sql"));
        assert!(!help.contains(&format!("{}-{}", "pgsandbox", "mcp")));
    }

    #[test]
    fn cli_tool_commands_cover_public_mcp_tools() {
        for tool_name in crate::mcp::PUBLIC_MCP_TOOLS {
            assert!(
                CLI_TOOL_COMMANDS
                    .iter()
                    .any(|command| command.tool_name == *tool_name),
                "missing CLI command for MCP tool {tool_name}"
            );
        }
    }

    #[test]
    fn cli_tool_invocation_accepts_hyphenated_command_json_input() {
        let invocation = parse_cli_tool_invocation(
            "create-database",
            &args(&[
                "--input",
                r#"{"nameHint":"migration check","ttlMinutes":15}"#,
            ]),
        )
        .unwrap();

        assert_eq!(invocation.tool_name, "create_database");
        assert_eq!(invocation.input["nameHint"], "migration check");
        assert_eq!(invocation.input["ttlMinutes"], 15);
    }

    #[test]
    fn cli_tool_invocation_merges_common_flags_into_json_input() {
        let invocation = parse_cli_tool_invocation(
            "run-sql",
            &args(&[
                "--database-id",
                "db-123",
                "--sql",
                "select 1",
                "--readonly",
                "--row-limit",
                "5",
            ]),
        )
        .unwrap();

        assert_eq!(invocation.tool_name, "run_sql");
        assert_eq!(invocation.input["databaseId"], "db-123");
        assert_eq!(invocation.input["sql"], "select 1");
        assert_eq!(invocation.input["readonly"], true);
        assert_eq!(invocation.input["rowLimit"], 5);
    }

    #[test]
    fn cli_tool_invocation_keeps_postgres_version_as_a_string() {
        let invocation = parse_cli_tool_invocation(
            "create-database",
            &args(&["--postgres-version", "18", "--name-hint", "versioned"]),
        )
        .unwrap();

        assert_eq!(invocation.input["postgresVersion"], "18");

        let equals_invocation =
            parse_cli_tool_invocation("create-database", &args(&["--postgres-version=18"]))
                .unwrap();

        assert_eq!(equals_invocation.input["postgresVersion"], "18");
    }

    #[test]
    fn help_text_lists_upgrade_command() {
        let help = help_text();

        assert!(help.contains("pgsandbox upgrade [options]"));
        assert!(help.contains("--setup <client>"));
        assert!(help.contains("--no-setup"));
    }

    #[test]
    fn help_text_lists_uninstall_command() {
        let help = help_text();

        assert!(help.contains("pgsandbox uninstall [options]"));
        assert!(help.contains("--keep-client-config"));
    }

    #[test]
    fn help_text_lists_ensure_postgres_command() {
        let help = help_text();

        assert!(help.contains("pgsandbox ensure-postgres"));
        assert!(help.contains("--no-install"));
        assert!(help.contains("--install-missing"));
    }

    #[test]
    fn help_text_lists_extension_discovery_command() {
        let help = help_text();

        assert!(help.contains("pgsandbox list-extensions [options]"));
        assert!(help.contains("--database-id <id>"));
        assert!(help.contains("--database-name <name>"));
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
    fn parse_options_accepts_postgres_install_boolean_flags() {
        let options = parse_options(&args(&["--install-missing", "--no-install"])).unwrap();

        assert_eq!(
            options.get("install-missing").map(String::as_str),
            Some("true")
        );
        assert_eq!(options.get("no-install").map(String::as_str), Some("true"));
    }

    #[test]
    fn parse_options_accepts_smoke_test_verbose_flag() {
        let options = parse_options(&args(&["--verbose"])).unwrap();

        assert_eq!(options.get("verbose").map(String::as_str), Some("true"));
    }

    #[test]
    fn smoke_test_results_are_concise_unless_verbose() {
        let output = crate::postgres::RunSqlOutput {
            database_id: "db-id".to_string(),
            database_name: "pgsandbox_smoke_123".to_string(),
            returned_row_count: 1,
            affected_row_count: Some(1),
            total_row_count_known: true,
            rows: vec![serde_json::json!({"name": "alpha"})],
            result_sets: Vec::new(),
            truncated: false,
            elapsed_ms: 12,
        };

        let concise = render_smoke_test_result("insert serialization", &output, false).unwrap();
        let verbose = render_smoke_test_result("insert serialization", &output, true).unwrap();

        assert_eq!(
            concise,
            "PASS insert serialization (rows=1, affected=1, 12ms)"
        );
        assert!(!concise.contains("databaseId"));
        assert!(verbose.contains("\"databaseId\": \"db-id\""));
        assert!(verbose.contains("\"name\": \"alpha\""));
    }

    #[test]
    fn detects_cargo_or_source_upgrade_path() {
        let plan = detect_upgrade_plan(Path::new("/repo/target/debug/pgsandbox"));

        assert_eq!(plan.source, UpgradeInstallSource::Cargo);
    }

    #[test]
    fn detects_direct_release_upgrade_path() {
        let plan = detect_upgrade_plan(Path::new("/home/user/.local/bin/pgsandbox"));

        assert_eq!(plan.source, UpgradeInstallSource::DirectRelease);
    }

    #[test]
    fn detects_homebrew_cellar_path_shape() {
        assert!(path_has_homebrew_cellar_pgsandbox(Path::new(
            "/opt/homebrew/Cellar/pgsandbox/0.4.5/bin/pgsandbox"
        )));
    }

    #[test]
    fn homebrew_upgrade_rejects_pinned_version() {
        let options = BTreeMap::from([("version".to_string(), "0.4.5".to_string())]);
        let error = ensure_homebrew_upgrade_options(&options).unwrap_err();

        assert!(error.to_string().contains("--version is only supported"));
    }

    #[test]
    fn homebrew_upgrade_allows_default_options() {
        let options = BTreeMap::new();

        ensure_homebrew_upgrade_options(&options).unwrap();
    }

    #[test]
    fn upgrade_setup_args_default_to_all_user_with_absolute_command() {
        let options = BTreeMap::from([("postgres-version".to_string(), "18".to_string())]);
        let setup_args = upgrade_setup_args(
            &options,
            crate::setup::ClientSelector::All,
            crate::setup::ConfigScope::User,
            "/usr/local/bin/pgsandbox",
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
                "/usr/local/bin/pgsandbox",
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
    fn setup_installs_unversioned_postgres_by_default() {
        let plan = crate::local::postgres_install_plan_for_manager(
            crate::local::LocalPostgresPackageManager::Homebrew,
            None,
        )
        .unwrap();

        assert_eq!(plan.package, "postgresql");
        assert_eq!(plan.display_command, "brew install postgresql");
    }

    #[test]
    fn setup_installs_requested_postgres_major_version() {
        let plan = crate::local::postgres_install_plan_for_manager(
            crate::local::LocalPostgresPackageManager::Homebrew,
            Some("18.4"),
        )
        .unwrap();

        assert_eq!(plan.package, "postgresql@18");
        assert_eq!(plan.display_command, "brew install postgresql@18");
    }

    #[test]
    fn setup_treats_runtime_missing_binary_message_as_installable() {
        let error = anyhow::anyhow!(
            "Postgres server binaries `initdb`, `pg_ctl`, and `postgres` were not found together on PATH or in common local install locations."
        );

        assert!(crate::local::local_postgres_error_is_missing_binaries(
            &error
        ));
    }

    #[test]
    fn help_text_describes_setup_preflight() {
        let help = help_text();

        assert!(help.contains("Check and start managed local Postgres"));
    }
}
