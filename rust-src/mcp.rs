use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router, ErrorData, ServiceExt,
};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::{
    config::SandboxConfig,
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

#[derive(Clone)]
pub struct PgsandboxServer {
    manager: PostgresSandboxManager,
    telemetry: Telemetry,
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

    #[tool(description = "Return schema metadata for a sandbox database.")]
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

    #[tool(description = "Return a compact checksummed schema digest for a sandbox database.")]
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

    #[tool(description = "Compare a base schema_digest response with the current sandbox schema.")]
    async fn schema_diff(
        &self,
        Parameters(input): Parameters<SchemaDiffInput>,
    ) -> Result<CallToolResult, ErrorData> {
        let event_properties = properties([
            ("hasProfile", json!(input.profile.is_some())),
            ("hasDatabaseId", json!(input.database_id.is_some())),
            ("hasDatabaseName", json!(input.database_name.is_some())),
            ("hasBaseDigest", json!(!input.base_digest.is_null())),
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

    #[tool(description = "Create a named schema snapshot for a PGSandbox-owned database.")]
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

    #[tool(description = "List schema snapshots for a PGSandbox-owned database.")]
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

    #[tool(description = "Delete a named schema snapshot for a PGSandbox-owned database.")]
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

    #[tool(description = "Diff a named schema snapshot against the current sandbox schema.")]
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

    #[tool(description = "Detect a Django repo and write a secret-free PG Sandbox project config.")]
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

    #[tool(description = "Run an explicit Django migration command against a sandbox database.")]
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

    #[tool(description = "Run Django migrations in a sandbox and return before/after schema diff.")]
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

    #[tool(description = "Export a PGSandbox-owned database to a local template artifact.")]
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

    #[tool(description = "Create a new sandbox database from a local template artifact.")]
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

    #[tool(description = "List local template artifacts for a profile.")]
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

    #[tool(description = "Delete a local template artifact.")]
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
            ("dryRun", json!(input.dry_run.unwrap_or(false))),
        ]);
        self.tracked_tool(
            "cleanup_expired",
            event_properties,
            self.manager.cleanup_expired(input),
        )
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
        Err(error) => Ok(CallToolResult::error(vec![Content::text(
            error.to_string(),
        )])),
    }
}

fn internal_error(error: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}
