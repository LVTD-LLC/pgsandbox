use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router, ErrorData, ServiceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    config::SandboxConfig,
    doctor::run_doctor,
    postgres::{
        CleanupExpiredInput, CloneDatabaseInput, CreateDatabaseInput,
        CreateSandboxFromTemplateInput, CreateSchemaSnapshotInput, CreateTemplateFromSandboxInput,
        DatabaseSelector, DeleteSchemaSnapshotInput, DeleteTemplateInput, DiffSchemaSnapshotInput,
        ExplainQueryInput, ListDatabasesInput, ListProfilesInput, ListSchemaSnapshotsInput,
        ListTemplatesInput, PostgresSandboxManager, PrepareForRepoInput, RunMigrationsInput,
        RunSqlInput, SchemaDiffInput, SeedDatabaseInput, ValidateMigrationInput,
    },
    telemetry::{properties, Telemetry, EVENT_MCP_SERVER_STARTED, EVENT_MCP_TOOL_COMPLETED},
};

pub const PUBLIC_MCP_TOOLS: &[&str] = &[
    "list_profiles",
    "create_database",
    "clone_database",
    "delete_database",
    "get_connection_string",
    "run_sql",
    "describe_schema",
    "schema_digest",
    "schema_diff",
    "explain_query",
    "create_schema_snapshot",
    "list_schema_snapshots",
    "delete_schema_snapshot",
    "diff_schema_snapshot",
    "prepare_for_repo",
    "run_repo_command",
    "run_migrations",
    "validate_schema_change",
    "validate_migration",
    "seed_database",
    "create_template_from_sandbox",
    "create_sandbox_from_template",
    "list_templates",
    "delete_template",
    "list_databases",
    "cleanup_expired",
    "doctor",
];

pub const PUBLIC_MCP_TOOL_COUNT: usize = PUBLIC_MCP_TOOLS.len();

#[derive(Clone)]
pub struct PgsandboxServer {
    manager: PostgresSandboxManager,
    telemetry: Telemetry,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct DoctorInput {
    postgres_version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DoctorOutput {
    ok: bool,
    server_version: String,
    tool_count: usize,
    lines: Vec<String>,
}

impl PgsandboxServer {
    pub fn new(config: SandboxConfig) -> Self {
        let telemetry = Telemetry::new(config.telemetry.clone());
        Self {
            manager: PostgresSandboxManager::new(config),
            telemetry,
        }
    }

    async fn tracked_tool<T, Fut>(
        &self,
        tool: &'static str,
        mut event_properties: Map<String, Value>,
        operation: Fut,
    ) -> Result<CallToolResult, ErrorData>
    where
        T: Serialize,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        let started = std::time::Instant::now();
        let result = operation.await;
        event_properties.insert("tool".to_string(), json!(tool));
        event_properties.insert("success".to_string(), json!(result.is_ok()));
        event_properties.insert(
            "elapsedMs".to_string(),
            json!(started.elapsed().as_millis()),
        );
        self.telemetry
            .capture_background(EVENT_MCP_TOOL_COMPLETED, event_properties);
        tool_json(result)
    }
}

#[tool_router(server_handler)]
impl PgsandboxServer {
    #[tool(description = "List configured and discoverable Postgres profiles.")]
    async fn list_profiles(
        &self,
        Parameters(input): Parameters<ListProfilesInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([(
            "includeDiscoveredLocal",
            json!(input.include_discovered_local.unwrap_or(true)),
        )]);
        self.tracked_tool("list_profiles", event_properties, async {
            self.manager.list_profiles(input)
        })
        .await
    }

    #[tool(description = "Create an isolated Postgres sandbox database and login role.")]
    async fn create_database(
        &self,
        Parameters(input): Parameters<CreateDatabaseInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            (
                "hasPostgresVersion",
                json!(input.postgres_version.is_some()),
            ),
            ("hasNameHint", json!(input.name_hint.is_some())),
            ("hasOwner", json!(input.owner.is_some())),
            ("hasTtlMinutes", json!(input.ttl_minutes.is_some())),
            (
                "labelCount",
                json!(input.labels.as_ref().map_or(0, |labels| labels.len())),
            ),
        ]);
        self.tracked_tool(
            "create_database",
            event_properties,
            self.manager.create_database(input),
        )
        .await
    }

    #[tool(description = "Clone an existing Postgres database into a new isolated sandbox.")]
    async fn clone_database(
        &self,
        Parameters(input): Parameters<CloneDatabaseInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            (
                "hasPostgresVersion",
                json!(input.postgres_version.is_some()),
            ),
            ("hasNameHint", json!(input.name_hint.is_some())),
            ("hasOwner", json!(input.owner.is_some())),
            ("hasTtlMinutes", json!(input.ttl_minutes.is_some())),
            (
                "labelCount",
                json!(input.labels.as_ref().map_or(0, |labels| labels.len())),
            ),
            ("schemaOnly", json!(input.schema_only.unwrap_or(false))),
        ]);
        self.tracked_tool(
            "clone_database",
            event_properties,
            self.manager.clone_database(input),
        )
        .await
    }

    #[tool(description = "Delete a sandbox database and role created by PGSandbox.")]
    async fn delete_database(
        &self,
        Parameters(input): Parameters<DatabaseSelector>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = selector_properties(&input);
        self.tracked_tool(
            "delete_database",
            event_properties,
            self.manager.delete_database(input),
        )
        .await
    }

    #[tool(description = "Return the connection string for a sandbox database.")]
    async fn get_connection_string(
        &self,
        Parameters(input): Parameters<DatabaseSelector>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = selector_properties(&input);
        self.tracked_tool(
            "get_connection_string",
            event_properties,
            self.manager.get_connection_string(input),
        )
        .await
    }

    #[tool(description = "Run SQL against a sandbox database.")]
    async fn run_sql(
        &self,
        Parameters(input): Parameters<RunSqlInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
            ("readonly", json!(input.readonly.unwrap_or(false))),
            ("hasRowLimit", json!(input.row_limit.is_some())),
        ]);
        self.tracked_tool("run_sql", event_properties, self.manager.run_sql(input))
            .await
    }

    #[tool(
        description = "Inspect sandbox schema metadata for tables, views, relation kinds, columns, defaults, constraints, indexes, extensions, and migration review."
    )]
    async fn describe_schema(
        &self,
        Parameters(input): Parameters<DatabaseSelector>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = selector_properties(&input);
        self.tracked_tool(
            "describe_schema",
            event_properties,
            self.manager.describe_schema(input),
        )
        .await
    }

    #[tool(
        description = "Create a compact checksummed schema digest for schema diff, migration review, before/after comparison, snapshots, and drift detection."
    )]
    async fn schema_digest(
        &self,
        Parameters(input): Parameters<DatabaseSelector>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = selector_properties(&input);
        self.tracked_tool(
            "schema_digest",
            event_properties,
            self.manager.schema_digest(input),
        )
        .await
    }

    #[tool(
        description = "Compare a prior schema_digest response with the current sandbox schema for before/after schema diff, migration review, and drift detection."
    )]
    async fn schema_diff(
        &self,
        Parameters(input): Parameters<SchemaDiffInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
        ]);
        self.tracked_tool(
            "schema_diff",
            event_properties,
            self.manager.schema_diff(input),
        )
        .await
    }

    #[tool(
        description = "Return a JSON EXPLAIN plan and compact summary for one sandbox SQL statement."
    )]
    async fn explain_query(
        &self,
        Parameters(input): Parameters<ExplainQueryInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
        ]);
        self.tracked_tool(
            "explain_query",
            event_properties,
            self.manager.explain_query(input),
        )
        .await
    }

    #[tool(
        description = "Create a named schema snapshot checkpoint for before/after migration review, schema diff workflows, rollback comparison, and drift detection."
    )]
    async fn create_schema_snapshot(
        &self,
        Parameters(input): Parameters<CreateSchemaSnapshotInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
            ("hasNotes", json!(input.notes.is_some())),
        ]);
        self.tracked_tool(
            "create_schema_snapshot",
            event_properties,
            self.manager.create_schema_snapshot(input),
        )
        .await
    }

    #[tool(
        description = "List named schema snapshot checkpoints for before/after migration review, schema diff workflows, and stored schema baselines."
    )]
    async fn list_schema_snapshots(
        &self,
        Parameters(input): Parameters<ListSchemaSnapshotsInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
        ]);
        self.tracked_tool(
            "list_schema_snapshots",
            event_properties,
            self.manager.list_schema_snapshots(input),
        )
        .await
    }

    #[tool(
        description = "Delete a named schema snapshot checkpoint created for schema diff workflows or migration review baselines."
    )]
    async fn delete_schema_snapshot(
        &self,
        Parameters(input): Parameters<DeleteSchemaSnapshotInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
        ]);
        self.tracked_tool(
            "delete_schema_snapshot",
            event_properties,
            self.manager.delete_schema_snapshot(input),
        )
        .await
    }

    #[tool(
        description = "Diff a named schema snapshot against the current sandbox schema for before/after migration review, drift detection, and schema comparison workflows."
    )]
    async fn diff_schema_snapshot(
        &self,
        Parameters(input): Parameters<DiffSchemaSnapshotInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
        ]);
        self.tracked_tool(
            "diff_schema_snapshot",
            event_properties,
            self.manager.diff_schema_snapshot(input),
        )
        .await
    }

    #[tool(description = "Prepare generic repo workflow metadata for PG Sandbox.")]
    async fn prepare_for_repo(
        &self,
        Parameters(input): Parameters<PrepareForRepoInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
        ]);
        self.tracked_tool(
            "prepare_for_repo",
            event_properties,
            self.manager.prepare_for_repo(input),
        )
        .await
    }

    #[tool(
        description = "Run an explicit repo command against a sandbox database with DATABASE_URL and PG* env vars scoped to the sandbox."
    )]
    async fn run_repo_command(
        &self,
        Parameters(input): Parameters<RunMigrationsInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
            ("hasCommand", json!(input.command.is_some())),
            ("hasTimeout", json!(input.timeout_seconds.is_some())),
        ]);
        self.tracked_tool(
            "run_repo_command",
            event_properties,
            self.manager.run_repo_command(input),
        )
        .await
    }

    #[tool(
        description = "Run an explicit repo schema-change command against a sandbox database. Backward-compatible alias for generic repo command workflows."
    )]
    async fn run_migrations(
        &self,
        Parameters(input): Parameters<RunMigrationsInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
            ("hasCommand", json!(input.command.is_some())),
            ("hasTimeout", json!(input.timeout_seconds.is_some())),
        ]);
        self.tracked_tool(
            "run_migrations",
            event_properties,
            self.manager.run_migrations(input),
        )
        .await
    }

    #[tool(
        description = "Run an explicit repo schema-change command in a sandbox and return a before/after schema diff."
    )]
    async fn validate_schema_change(
        &self,
        Parameters(input): Parameters<ValidateMigrationInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
            ("hasCommand", json!(input.command.is_some())),
            ("hasTimeout", json!(input.timeout_seconds.is_some())),
            ("hasTtlMinutes", json!(input.ttl_minutes.is_some())),
            ("hasOwner", json!(input.owner.is_some())),
            (
                "labelCount",
                json!(input.labels.as_ref().map_or(0, |labels| labels.len())),
            ),
        ]);
        self.tracked_tool(
            "validate_schema_change",
            event_properties,
            self.manager.validate_schema_change(input),
        )
        .await
    }

    #[tool(
        description = "Run an explicit repo schema-change command in a sandbox and return before/after schema diff. Backward-compatible alias for migration workflows."
    )]
    async fn validate_migration(
        &self,
        Parameters(input): Parameters<ValidateMigrationInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
            ("hasCommand", json!(input.command.is_some())),
            ("hasTimeout", json!(input.timeout_seconds.is_some())),
            ("hasTtlMinutes", json!(input.ttl_minutes.is_some())),
            ("hasOwner", json!(input.owner.is_some())),
            (
                "labelCount",
                json!(input.labels.as_ref().map_or(0, |labels| labels.len())),
            ),
        ]);
        self.tracked_tool(
            "validate_migration",
            event_properties,
            self.manager.validate_migration(input),
        )
        .await
    }

    #[tool(description = "Run an explicit seed command against a sandbox database.")]
    async fn seed_database(
        &self,
        Parameters(input): Parameters<SeedDatabaseInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
            ("hasCommand", json!(input.command.is_some())),
            ("hasTimeout", json!(input.timeout_seconds.is_some())),
        ]);
        self.tracked_tool(
            "seed_database",
            event_properties,
            self.manager.seed_database(input),
        )
        .await
    }

    #[tool(
        description = "Export a PGSandbox-owned database to a reusable local template artifact for seeded sandbox workflows, regression fixtures, and pg_dump/pg_restore reuse."
    )]
    async fn create_template_from_sandbox(
        &self,
        Parameters(input): Parameters<CreateTemplateFromSandboxInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
            ("hasCreatedBy", json!(input.created_by.is_some())),
            ("hasNotes", json!(input.notes.is_some())),
        ]);
        self.tracked_tool(
            "create_template_from_sandbox",
            event_properties,
            self.manager.create_template_from_sandbox(input),
        )
        .await
    }

    #[tool(
        description = "Create a new sandbox database from a reusable local template artifact for seeded sandbox workflows, regression fixtures, and repeatable test states."
    )]
    async fn create_sandbox_from_template(
        &self,
        Parameters(input): Parameters<CreateSandboxFromTemplateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasNameHint", json!(input.name_hint.is_some())),
            ("hasOwner", json!(input.owner.is_some())),
            ("hasTtlMinutes", json!(input.ttl_minutes.is_some())),
            (
                "labelCount",
                json!(input.labels.as_ref().map_or(0, |labels| labels.len())),
            ),
        ]);
        self.tracked_tool(
            "create_sandbox_from_template",
            event_properties,
            self.manager.create_sandbox_from_template(input),
        )
        .await
    }

    #[tool(
        description = "List reusable local template artifacts for seeded sandbox workflows, template restore, regression fixtures, and repeatable test states."
    )]
    async fn list_templates(
        &self,
        Parameters(input): Parameters<ListTemplatesInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            (
                "hasPostgresVersion",
                json!(input.postgres_version.is_some()),
            ),
        ]);
        self.tracked_tool(
            "list_templates",
            event_properties,
            self.manager.list_templates(input),
        )
        .await
    }

    #[tool(
        description = "Delete a reusable local template artifact from seeded sandbox workflows and template restore state."
    )]
    async fn delete_template(
        &self,
        Parameters(input): Parameters<DeleteTemplateInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            (
                "hasPostgresVersion",
                json!(input.postgres_version.is_some()),
            ),
        ]);
        self.tracked_tool(
            "delete_template",
            event_properties,
            self.manager.delete_template(input),
        )
        .await
    }

    #[tool(description = "List sandbox databases known to PGSandbox.")]
    async fn list_databases(
        &self,
        Parameters(input): Parameters<ListDatabasesInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            (
                "hasPostgresVersion",
                json!(input.postgres_version.is_some()),
            ),
            (
                "includeAllVersions",
                json!(
                    input.include_all_versions.unwrap_or(false)
                        || input
                            .postgres_version
                            .as_deref()
                            .is_some_and(|version| version.trim() == "*")
                ),
            ),
            ("hasOwnerFilter", json!(input.owner.is_some())),
        ]);
        self.tracked_tool(
            "list_databases",
            event_properties,
            self.manager.list_databases(input),
        )
        .await
    }

    #[tool(description = "Delete expired sandbox databases.")]
    async fn cleanup_expired(
        &self,
        Parameters(input): Parameters<CleanupExpiredInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            (
                "hasPostgresVersion",
                json!(input.postgres_version.is_some()),
            ),
            (
                "includeAllVersions",
                json!(
                    input.include_all_versions.unwrap_or(false)
                        || input
                            .postgres_version
                            .as_deref()
                            .is_some_and(|version| version.trim() == "*")
                ),
            ),
            ("dryRun", json!(input.dry_run.unwrap_or(false))),
        ]);
        self.tracked_tool(
            "cleanup_expired",
            event_properties,
            self.manager.cleanup_expired(input),
        )
        .await
    }

    #[tool(
        description = "Return MCP-safe version, profile health, and redacted doctor diagnostics without mutating sandboxes."
    )]
    async fn doctor(
        &self,
        Parameters(input): Parameters<DoctorInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([(
            "hasPostgresVersion",
            json!(input.postgres_version.is_some()),
        )]);
        self.tracked_tool("doctor", event_properties, async move {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let result = run_doctor(None, input.postgres_version.as_deref(), &cwd).await;
            anyhow::Ok(DoctorOutput {
                ok: result.ok,
                server_version: crate::VERSION.to_string(),
                tool_count: PUBLIC_MCP_TOOL_COUNT,
                lines: result.lines,
            })
        })
        .await
    }
}

pub async fn serve_stdio(config: SandboxConfig) -> anyhow::Result<()> {
    let profile_count = config.profiles.len();
    let server = PgsandboxServer::new(config);
    let telemetry = server.telemetry.clone();
    let service = server.serve(rmcp::transport::stdio()).await?;
    telemetry.capture_background(
        EVENT_MCP_SERVER_STARTED,
        properties([("profileCount", json!(profile_count))]),
    );
    service.waiting().await?;
    Ok(())
}

fn selector_properties(input: &DatabaseSelector) -> Map<String, Value> {
    properties([
        ("hasProfile", json!(input.profile.is_some())),
        (
            "hasPostgresVersion",
            json!(input.postgres_version.is_some()),
        ),
        ("hasDatabaseId", json!(input.database_id.is_some())),
        ("hasDatabaseName", json!(input.database_name.is_some())),
    ])
}

fn tool_json<T: Serialize>(result: anyhow::Result<T>) -> Result<CallToolResult, ErrorData> {
    match result {
        Ok(value) => {
            let text = serde_json::to_string_pretty(&value).map_err(internal_error)?;
            Ok(CallToolResult::success(vec![Content::text(text)]))
        }
        Err(error) => {
            let text = serde_json::to_string_pretty(&ToolErrorResponse::from_error(&error))
                .map_err(internal_error)?;
            Ok(CallToolResult::error(vec![Content::text(text)]))
        }
    }
}

fn internal_error(error: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolErrorResponse {
    ok: bool,
    error: ToolErrorBody,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolErrorBody {
    code: &'static str,
    category: &'static str,
    message: String,
    hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sqlstate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_version: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    detected_versions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail_handle: Option<Value>,
}

impl ToolErrorResponse {
    fn from_error(error: &anyhow::Error) -> Self {
        let chain = sanitize_error_message(&format!("{error:#}"));
        let lower = chain.to_ascii_lowercase();
        let body = if let Some(body) = postgres_db_error_body(error, &chain) {
            body
        } else if let Some(body) = stringly_sql_error_body(&lower, &chain) {
            body
        } else if lower.contains("basedigest string must contain")
            || lower.contains("basedigest must be a schema_digest response")
        {
            ToolErrorBody {
                code: "invalid_base_digest",
                category: "validation",
                message: chain,
                hint: "Pass baseDigest as the full schema_digest response object or a JSON string containing that full object. A checksum alone cannot compute a diff; use schema snapshots for compact stored baselines.".to_string(),
                sqlstate: None,
                requested_version: None,
                source_version: None,
                target_version: None,
                detected_versions: Vec::new(),
                detail_handle: Some(json!({
                    "type": "tool-contract",
                    "tool": "schema_diff",
                    "field": "baseDigest"
                })),
            }
        } else if lower.contains("password authentication failed")
            || lower.contains("authentication failed")
        {
            ToolErrorBody {
                code: "postgres_auth_failed",
                category: "postgres",
                message: chain,
                hint: "Run `pgsandbox-mcp doctor` to identify the active config source. If an MCP client config has a stale explicit PGSANDBOX_ADMIN_DATABASE_URL, run `pgsandbox-mcp setup --client <client>` without --admin-url, restart the MCP client, and retry.".to_string(),
                sqlstate: None,
                requested_version: None,
                source_version: None,
                target_version: None,
                detected_versions: Vec::new(),
                detail_handle: None,
            }
        } else if lower.contains("duplicate key value violates unique constraint") {
            ToolErrorBody {
                code: "constraint_violation",
                category: "constraint_violation",
                message: chain,
                hint: "The SQL violated a database constraint. Inspect the constraint name and adjust the input or query before retrying.".to_string(),
                sqlstate: None,
                requested_version: None,
                source_version: None,
                target_version: None,
                detected_versions: Vec::new(),
                detail_handle: None,
            }
        } else if lower.contains("read-only transaction")
            || lower.contains("readonly sql cannot include")
        {
            ToolErrorBody {
                code: "readonly_violation",
                category: "readonly_violation",
                message: chain,
                hint: "The request attempted a write or session change through a readonly path. Retry with readonly=false only if mutation is intended.".to_string(),
                sqlstate: None,
                requested_version: None,
                source_version: None,
                target_version: None,
                detected_versions: Vec::new(),
                detail_handle: None,
            }
        } else if lower.contains("database was not found in pgsandbox metadata") {
            ToolErrorBody {
                code: "database_not_found",
                category: "database_not_found",
                message: chain,
                hint: "Retry with the sandbox's profile or postgresVersion, or call list_databases with includeAllVersions=true to discover active sandboxes across versions.".to_string(),
                sqlstate: None,
                requested_version: None,
                source_version: None,
                target_version: None,
                detected_versions: Vec::new(),
                detail_handle: None,
            }
        } else if lower.contains("does not match requested postgresversion") {
            ToolErrorBody {
                code: "version_mismatch",
                category: "version_mismatch",
                message: chain.clone(),
                hint: "When selecting by postgresVersion, omit profile unless intentionally targeting an exact versioned profile. Use list_profiles to inspect profile/version pairs.".to_string(),
                sqlstate: None,
                requested_version: requested_version_from_message(&chain),
                source_version: None,
                target_version: None,
                detected_versions: detected_postgres_versions(),
                detail_handle: Some(json!({
                    "type": "diagnostic",
                    "tool": "list_profiles"
                })),
            }
        } else if lower.contains("restore_incompatible") {
            let (source_version, target_version) =
                restore_incompatible_versions_from_message(&chain);
            ToolErrorBody {
                code: "restore_incompatible",
                category: "restore_incompatible",
                message: chain.clone(),
                hint: "Clone into the same or newer target Postgres major version, or create a dump that is compatible with the older target.".to_string(),
                sqlstate: None,
                requested_version: target_version
                    .clone()
                    .or_else(|| requested_version_from_message(&chain)),
                source_version,
                target_version,
                detected_versions: detected_postgres_versions(),
                detail_handle: Some(json!({
                    "type": "diagnostic",
                    "tool": "list_profiles"
                })),
            }
        } else if lower.contains("could not find local postgres")
            || lower.contains("failed to prepare local postgres profile")
            || lower.contains("failed to prepare default local postgres profile")
        {
            let requested_version = requested_version_from_message(&chain);
            ToolErrorBody {
                code: "local_postgres_unavailable",
                category: "local_postgres",
                message: requested_version
                    .as_ref()
                    .map(|version| format!("Local Postgres {version} binaries are unavailable."))
                    .unwrap_or_else(|| "Local Postgres binaries are unavailable.".to_string()),
                hint: "Install local PostgreSQL server binaries for the requested major version, set PGSANDBOX_POSTGRES_BIN_DIR or PGSANDBOX_POSTGRES_<MAJOR>_BIN_DIR, or choose a version shown by list_profiles.".to_string(),
                sqlstate: None,
                requested_version,
                source_version: None,
                target_version: None,
                detected_versions: detected_postgres_versions(),
                detail_handle: Some(json!({
                    "type": "diagnostic",
                    "command": "pgsandbox-mcp doctor"
                })),
            }
        } else if lower.contains("no configured profile advertises postgresversion")
            || lower.contains("unknown postgres version")
        {
            let requested_version = requested_version_from_message(&chain);
            ToolErrorBody {
                code: "postgres_version_unavailable",
                category: "config",
                message: requested_version
                    .as_ref()
                    .map(|version| {
                        format!("No configured profile advertises postgresVersion {version}.")
                    })
                    .unwrap_or(chain),
                hint: "Use a postgresVersion listed by list_profiles, add a matching explicit profile, or rerun setup without --admin-url to use managed local version discovery.".to_string(),
                sqlstate: None,
                requested_version,
                source_version: None,
                target_version: None,
                detected_versions: detected_postgres_versions(),
                detail_handle: Some(json!({
                    "type": "diagnostic",
                    "tool": "list_profiles"
                })),
            }
        } else if lower.contains("connection refused")
            || lower.contains("connection timed out")
            || lower.contains("failed to connect")
        {
            ToolErrorBody {
                code: "postgres_connection_failed",
                category: "postgres",
                message: chain,
                hint: "Run `pgsandbox-mcp doctor` to verify the configured profile and connectivity. For managed local, try `pgsandbox-mcp local status` or `pgsandbox-mcp local start`.".to_string(),
                sqlstate: None,
                requested_version: None,
                source_version: None,
                target_version: None,
                detected_versions: Vec::new(),
                detail_handle: None,
            }
        } else {
            ToolErrorBody {
                code: "pgsandbox_tool_failed",
                category: "unknown",
                message: chain,
                hint: "Run `pgsandbox-mcp doctor` for a local diagnostic, then retry the tool with the same profile or postgresVersion.".to_string(),
                sqlstate: None,
                requested_version: None,
                source_version: None,
                target_version: None,
                detected_versions: Vec::new(),
                detail_handle: None,
            }
        };

        Self {
            ok: false,
            error: body,
        }
    }
}

fn postgres_db_error_body(error: &anyhow::Error, message: &str) -> Option<ToolErrorBody> {
    let postgres_error = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<tokio_postgres::Error>())?;
    let db_error = postgres_error.as_db_error()?;
    sqlstate_error_body(db_error.code().code(), message.to_string())
}

fn stringly_sql_error_body(lower: &str, message: &str) -> Option<ToolErrorBody> {
    let sqlstate = if lower.contains("column") && lower.contains("does not exist") {
        Some("42703")
    } else if lower.contains("relation") && lower.contains("does not exist") {
        Some("42P01")
    } else if lower.contains("syntax error") {
        Some("42601")
    } else if lower.contains("permission denied") {
        Some("42501")
    } else if lower.contains("statement timeout") {
        Some("57014")
    } else if lower.contains("lock timeout") || lower.contains("canceling statement due to lock") {
        Some("55P03")
    } else {
        None
    }?;
    sqlstate_error_body(sqlstate, message.to_string())
}

fn sqlstate_error_body(sqlstate: &str, message: String) -> Option<ToolErrorBody> {
    let (code, category, hint) = match sqlstate {
        "42703" => (
            "undefined_column",
            "sql_analysis",
            "The query references a column that does not exist. Call describe_schema or check identifier spelling/casing before retrying.",
        ),
        "42P01" => (
            "undefined_table",
            "sql_analysis",
            "The query references a table or relation that does not exist. Call describe_schema or verify the schema/search_path before retrying.",
        ),
        "42601" => (
            "syntax_error",
            "sql_syntax",
            "Revise the SQL syntax and retry. describe_schema can help confirm object names, but doctor is not needed for a syntax error.",
        ),
        "42501" => (
            "permission_denied",
            "permission_denied",
            "The sandbox role lacks permission for this operation. Use allowed sandbox operations or inspect object ownership/privileges.",
        ),
        "55P03" => (
            "lock_timeout",
            "timeout",
            "The statement could not acquire a lock in time. Retry after the conflicting transaction completes or inspect active sessions.",
        ),
        "57014" => (
            "statement_timeout",
            "timeout",
            "The statement was canceled by Postgres. Simplify the query, add a narrower predicate, or retry with a smaller operation.",
        ),
        state if state.starts_with("23") => (
            "constraint_violation",
            "constraint_violation",
            "The SQL violated a database constraint. Inspect the constraint name and adjust the input or query before retrying.",
        ),
        state if state.starts_with("08") => (
            "postgres_connection_failed",
            "postgres",
            "Run doctor to verify connectivity, then retry the tool with the same profile or postgresVersion.",
        ),
        "25006" => (
            "readonly_violation",
            "readonly_violation",
            "The request attempted a write through a readonly path. Retry with readonly=false only if mutation is intended.",
        ),
        _ => return None,
    };

    Some(ToolErrorBody {
        code,
        category,
        message,
        hint: hint.to_string(),
        sqlstate: Some(sqlstate.to_string()),
        requested_version: None,
        source_version: None,
        target_version: None,
        detected_versions: Vec::new(),
        detail_handle: None,
    })
}

fn detected_postgres_versions() -> Vec<String> {
    let mut versions = crate::local::discover_local_postgres_installations()
        .into_iter()
        .map(|installation| installation.postgres_version)
        .collect::<Vec<_>>();
    versions.sort();
    versions.dedup();
    versions
}

fn requested_version_from_message(message: &str) -> Option<String> {
    requested_version_after(message, "requested postgresVersion")
        .or_else(|| requested_version_after(message, "postgresVersion"))
        .or_else(|| requested_version_after(message, "Local Postgres"))
        .or_else(|| requested_version_after(message, "local Postgres"))
}

fn restore_incompatible_versions_from_message(message: &str) -> (Option<String>, Option<String>) {
    (
        requested_version_after(message, "from Postgres"),
        requested_version_after(message, "target Postgres"),
    )
}

fn requested_version_after(message: &str, marker: &str) -> Option<String> {
    let start = message.find(marker)? + marker.len();
    let version = message[start..]
        .trim_start_matches(|character: char| {
            character.is_whitespace() || matches!(character, ':' | '=' | '"' | '`')
        })
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    (!version.is_empty()).then_some(version)
}

fn sanitize_error_message(message: &str) -> String {
    let mut sanitized = String::with_capacity(message.len());
    let mut cursor = 0;

    while let Some(relative_start) = find_postgres_url_start(&message[cursor..]) {
        let start = cursor + relative_start;
        sanitized.push_str(&message[cursor..start]);

        let tail = &message[start..];
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        sanitized.push_str(&sanitize_error_token(&tail[..end]));
        cursor = start + end;
    }

    sanitized.push_str(&message[cursor..]);
    sanitized
}

fn find_postgres_url_start(message: &str) -> Option<usize> {
    let lower = message.to_ascii_lowercase();
    [lower.find("postgres://"), lower.find("postgresql://")]
        .into_iter()
        .flatten()
        .min()
}

fn sanitize_error_token(token: &str) -> String {
    let Some((scheme, rest)) = token.split_once("://") else {
        return token.to_string();
    };
    if !matches!(scheme, "postgres" | "postgresql") {
        return token.to_string();
    }
    let Some((credentials, suffix)) = rest.split_once('@') else {
        return token.to_string();
    };
    let Some((user, _password)) = credentials.split_once(':') else {
        return token.to_string();
    };
    format!("{scheme}://{user}:****@{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_errors_are_structured_and_actionable() {
        let result = tool_json::<()>(Err(anyhow::anyhow!(
            "failed to connect to Postgres admin profile default at postgresql://postgres:****@localhost:5432/postgres?sslmode=disable: db error: FATAL: password authentication failed for user \"postgres\""
        )))
        .unwrap();

        assert!(result.is_error.unwrap_or(false));
        let text = result.content[0].as_text().unwrap().text.clone();
        let value = serde_json::from_str::<Value>(&text).unwrap();

        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "postgres_auth_failed");
        assert_eq!(value["error"]["category"], "postgres");
        assert!(value["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("pgsandbox-mcp doctor"));
        assert!(!text.contains("postgres:postgres@"));
    }

    #[test]
    fn tool_errors_use_specific_agent_categories() {
        let cases = [
            (
                "db error: ERROR: duplicate key value violates unique constraint \"users_email_key\"",
                "constraint_violation",
                "constraint_violation",
            ),
            (
                "db error: ERROR: cannot execute INSERT in a read-only transaction",
                "readonly_violation",
                "readonly_violation",
            ),
            (
                "Database was not found in PGSandbox metadata.",
                "database_not_found",
                "database_not_found",
            ),
            (
                "profile local postgresVersion 18 does not match requested postgresVersion 16",
                "version_mismatch",
                "version_mismatch",
            ),
            (
                "restore_incompatible: cannot clone from Postgres 18 into older target Postgres 16",
                "restore_incompatible",
                "restore_incompatible",
            ),
            (
                "db error: ERROR: column \"definitely_missing_column\" does not exist",
                "undefined_column",
                "sql_analysis",
            ),
            (
                "db error: ERROR: syntax error at or near \"fromm\"",
                "syntax_error",
                "sql_syntax",
            ),
            (
                "baseDigest must be a schema_digest response: baseDigest string must contain the full JSON schema_digest response, not only the checksum",
                "invalid_base_digest",
                "validation",
            ),
        ];

        for (message, code, category) in cases {
            let result = tool_json::<()>(Err(anyhow::anyhow!(message))).unwrap();
            let text = result.content[0].as_text().unwrap().text.clone();
            let value = serde_json::from_str::<Value>(&text).unwrap();

            assert_eq!(value["error"]["code"], code);
            assert_eq!(value["error"]["category"], category);
        }
    }

    #[test]
    fn sqlstate_classifier_returns_agent_actionable_hints() {
        let undefined_column =
            sqlstate_error_body("42703", "ERROR: missing column".to_string()).unwrap();
        let syntax = sqlstate_error_body("42601", "ERROR: syntax error".to_string()).unwrap();
        let connection = sqlstate_error_body("08006", "connection failure".to_string()).unwrap();

        assert_eq!(undefined_column.code, "undefined_column");
        assert_eq!(undefined_column.sqlstate.as_deref(), Some("42703"));
        assert!(undefined_column.hint.contains("describe_schema"));
        assert_eq!(syntax.category, "sql_syntax");
        assert!(!syntax.hint.contains("doctor is needed"));
        assert_eq!(connection.code, "postgres_connection_failed");
    }

    #[test]
    fn unavailable_version_errors_are_compact_and_structured() {
        let result = tool_json::<()>(Err(anyhow::anyhow!(
            "failed to prepare local Postgres profile for postgresVersion 15: could not find local Postgres 15 binaries. Tried: /very/long/path/bin/initdb failed; /another/path/pg_ctl failed"
        )))
        .unwrap();
        let text = result.content[0].as_text().unwrap().text.clone();
        let value = serde_json::from_str::<Value>(&text).unwrap();

        assert_eq!(value["error"]["code"], "local_postgres_unavailable");
        assert_eq!(value["error"]["requestedVersion"], "15");
        assert_eq!(
            value["error"]["message"],
            "Local Postgres 15 binaries are unavailable."
        );
        assert!(value["error"]["detectedVersions"].is_array());
        assert!(value["error"]["detailHandle"].is_object());
        assert!(!text.contains("/very/long/path"));
    }

    #[test]
    fn restore_incompatible_errors_include_source_and_target_versions() {
        let result = tool_json::<()>(Err(anyhow::anyhow!(
            "restore_incompatible: cannot clone from Postgres 18 into older target Postgres 16"
        )))
        .unwrap();
        let text = result.content[0].as_text().unwrap().text.clone();
        let value = serde_json::from_str::<Value>(&text).unwrap();

        assert_eq!(value["error"]["code"], "restore_incompatible");
        assert_eq!(value["error"]["requestedVersion"], "16");
        assert_eq!(value["error"]["sourceVersion"], "18");
        assert_eq!(value["error"]["targetVersion"], "16");
    }

    #[test]
    fn sanitizes_postgres_urls_inside_punctuation() {
        let sanitized = sanitize_error_message(
            "failed (postgresql://postgres:secret@localhost:5432/postgres), retry 'postgres://pg:another-secret@127.0.0.1/db'",
        );

        assert!(sanitized.contains("(postgresql://postgres:****@localhost:5432/postgres),"));
        assert!(sanitized.contains("'postgres://pg:****@127.0.0.1/db'"));
        assert!(!sanitized.contains("secret@"));
        assert!(!sanitized.contains("another-secret@"));
    }

    #[test]
    fn public_tool_metadata_matches_tool_registrations() {
        let source = include_str!("mcp.rs");
        let tool_attribute = concat!("#[", "tool(");

        assert_eq!(
            PUBLIC_MCP_TOOL_COUNT,
            source.matches(tool_attribute).count(),
            "PUBLIC_MCP_TOOLS must be updated when adding or removing #[tool] registrations"
        );
        for tool_name in PUBLIC_MCP_TOOLS {
            assert!(
                source.contains(&format!("async fn {tool_name}(")),
                "PUBLIC_MCP_TOOLS contains {tool_name}, but no matching tool method exists"
            );
        }
    }
}
