use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router, ErrorData, ServiceExt,
};
use serde::Serialize;

use crate::{
    config::SandboxConfig,
    postgres::{
        CleanupExpiredInput, CreateDatabaseInput, DatabaseSelector, ListDatabasesInput,
        PostgresSandboxManager, RunSqlInput,
    },
};

#[derive(Clone)]
pub struct PgsandboxServer {
    manager: PostgresSandboxManager,
}

impl PgsandboxServer {
    pub fn new(config: SandboxConfig) -> Self {
        Self {
            manager: PostgresSandboxManager::new(config),
        }
    }
}

#[tool_router(server_handler)]
impl PgsandboxServer {
    #[tool(description = "Create an isolated Postgres sandbox database and login role.")]
    async fn create_database(
        &self,
        Parameters(input): Parameters<CreateDatabaseInput>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_json(self.manager.create_database(input).await)
    }

    #[tool(description = "Delete a sandbox database and role created by PGSandbox.")]
    async fn delete_database(
        &self,
        Parameters(input): Parameters<DatabaseSelector>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_json(self.manager.delete_database(input).await)
    }

    #[tool(description = "Return the connection string for a sandbox database.")]
    async fn get_connection_string(
        &self,
        Parameters(input): Parameters<DatabaseSelector>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_json(self.manager.get_connection_string(input).await)
    }

    #[tool(description = "Run SQL against a sandbox database.")]
    async fn run_sql(
        &self,
        Parameters(input): Parameters<RunSqlInput>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_json(self.manager.run_sql(input).await)
    }

    #[tool(description = "Return schema metadata for a sandbox database.")]
    async fn describe_schema(
        &self,
        Parameters(input): Parameters<DatabaseSelector>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_json(self.manager.describe_schema(input).await)
    }

    #[tool(description = "List sandbox databases known to PGSandbox.")]
    async fn list_databases(
        &self,
        Parameters(input): Parameters<ListDatabasesInput>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_json(self.manager.list_databases(input).await)
    }

    #[tool(description = "Delete expired sandbox databases.")]
    async fn cleanup_expired(
        &self,
        Parameters(input): Parameters<CleanupExpiredInput>,
    ) -> Result<CallToolResult, ErrorData> {
        tool_json(self.manager.cleanup_expired(input).await)
    }
}

pub async fn serve_stdio(config: SandboxConfig) -> anyhow::Result<()> {
    let service = PgsandboxServer::new(config)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
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
