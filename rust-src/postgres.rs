use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::LazyLock,
    time::Duration as StdDuration,
};

use aes_gcm::{
    aead::{Aead, AeadCore, OsRng},
    Aes256Gcm, KeyInit, Nonce,
};
use anyhow::Context;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use postgres_native_tls::MakeTlsConnector;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time,
};
use tokio_postgres::{
    error::SqlState,
    types::{FromSql, ToSql, Type},
    Client, NoTls, Row, SimpleQueryMessage,
};
use url::Url;
use uuid::Uuid;

use crate::{
    config::{
        find_profile, find_profile_for_request, ConfigError, SandboxConfig, SandboxProfile,
        DEFERRED_LOCAL_ADMIN_URL,
    },
    local::{discover_local_postgres_installations, LocalClusterConfig, LocalPostgresCluster},
    mcp::PUBLIC_MCP_TOOL_COUNT,
    names::{make_sandbox_names, quote_ident, quote_literal},
};

const METADATA_TABLE: &str = "pgsandbox_databases";
const AUDIT_TABLE: &str = "pgsandbox_events";
const DEFAULT_ROW_LIMIT: usize = 100;
const LIST_DATABASES_LIMIT: usize = 100;
const ENCRYPTED_PASSWORD_PREFIX: &str = "v1";
const SCHEMA_DIGEST_VERSION: u32 = 2;
const MAX_SCHEMA_DIFF_ITEMS: usize = 50;
const DEFAULT_WORKFLOW_TIMEOUT_SECONDS: u64 = 120;
const MAX_WORKFLOW_TIMEOUT_SECONDS: u64 = 600;
const DEFAULT_SCHEMA_OPERATION_TIMEOUT_SECONDS: u64 = 30;
const CONNECTION_TASK_CLOSE_TIMEOUT_SECONDS: u64 = 2;
const MAX_COMMAND_OUTPUT_BYTES: usize = 8_000;
const MAX_WORKFLOW_COMMAND_PARTS: usize = 16;
const MAX_WORKFLOW_COMMAND_TOTAL_BYTES: usize = 2_048;
const MAX_WORKFLOW_COMMAND_PART_BYTES: usize = 256;
const TEMPLATE_PRIVACY_WARNING: &str =
    "Templates are local PG Sandbox artifacts. Do not create templates from production or sensitive data unless you have explicitly sanitized it.";

static CURSOR_QUERY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)^\s*(?:--[^\n]*(?:\n|$)|/\*.*?\*/\s*)*(select|with|values|table)\b")
        .expect("cursor query regex compiles")
});

static TYPED_ROW_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)^\s*(?:--[^\n]*(?:\n|$)|/\*.*?\*/\s*)*(show|explain)\b")
        .expect("typed row prefix regex compiles")
});

#[derive(Clone)]
pub struct PostgresSandboxManager {
    config: SandboxConfig,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateDatabaseInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub name_hint: Option<String>,
    pub ttl_minutes: Option<u32>,
    pub owner: Option<String>,
    pub labels: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CloneDatabaseInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub source_database_url: String,
    pub name_hint: Option<String>,
    pub ttl_minutes: Option<u32>,
    pub owner: Option<String>,
    pub labels: Option<BTreeMap<String, Value>>,
    pub schema_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseSelector {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunSqlInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub sql: String,
    pub readonly: Option<bool>,
    pub row_limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListDatabasesInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub include_all_versions: Option<bool>,
    pub owner: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CleanupExpiredInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub include_all_versions: Option<bool>,
    pub dry_run: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDiffInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    #[schemars(
        description = "Full schema_digest response object, or a JSON string containing that full object. A checksum string alone is not enough to compute a diff; use schema snapshots for compact stored baselines."
    )]
    pub base_digest: SchemaDiffBaseDigest,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExplainQueryInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub sql: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSchemaSnapshotInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub snapshot_name: String,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListSchemaSnapshotsInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSchemaSnapshotInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub snapshot_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiffSchemaSnapshotInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub snapshot_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrepareForRepoInput {
    pub repo_path: String,
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub migration_command: Option<Vec<String>>,
    pub seed_command: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunRepoCommandInput {
    pub repo_path: String,
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub command: Option<Vec<String>>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidateSchemaChangeInput {
    pub repo_path: String,
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub command: Option<Vec<String>>,
    pub timeout_seconds: Option<u64>,
    pub name_hint: Option<String>,
    pub ttl_minutes: Option<u32>,
    pub owner: Option<String>,
    pub labels: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SeedDatabaseInput {
    pub repo_path: String,
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub command: Option<Vec<String>>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTemplateFromSandboxInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub template_name: String,
    pub created_by: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSandboxFromTemplateInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub template_name: String,
    pub name_hint: Option<String>,
    pub ttl_minutes: Option<u32>,
    pub owner: Option<String>,
    pub labels: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListTemplatesInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteTemplateInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub template_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListProfilesInput {
    pub include_discovered_local: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDatabasesOutput {
    pub scope: String,
    pub profiles: Vec<String>,
    pub databases: Vec<Value>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListProfilesOutput {
    pub server_version: String,
    pub tool_count: usize,
    pub restart_required_after_setup_note: String,
    pub available_postgres_versions: Vec<String>,
    pub hints: Vec<String>,
    pub profiles: Vec<ProfileSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub name: String,
    pub default: bool,
    pub postgres_version: Option<String>,
    pub port: Option<u16>,
    pub managed_local: bool,
    pub admin_url: String,
    pub source: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDatabaseOutput {
    pub database_id: String,
    pub profile: String,
    pub database_name: String,
    pub role_name: String,
    pub expires_at: DateTime<Utc>,
    #[serde(skip_serializing)]
    pub connection_string: String,
    pub connection_string_redacted: String,
}

impl fmt::Debug for CreateDatabaseOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreateDatabaseOutput")
            .field("database_id", &self.database_id)
            .field("profile", &self.profile)
            .field("database_name", &self.database_name)
            .field("role_name", &self.role_name)
            .field("expires_at", &self.expires_at)
            .field("connection_string", &"<redacted>")
            .field(
                "connection_string_redacted",
                &self.connection_string_redacted,
            )
            .finish()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneDatabaseOutput {
    pub database_id: String,
    pub profile: String,
    pub database_name: String,
    pub role_name: String,
    pub expires_at: DateTime<Utc>,
    #[serde(skip_serializing)]
    pub connection_string: String,
    pub connection_string_redacted: String,
    pub source: String,
    pub schema_only: bool,
}

impl fmt::Debug for CloneDatabaseOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloneDatabaseOutput")
            .field("database_id", &self.database_id)
            .field("profile", &self.profile)
            .field("database_name", &self.database_name)
            .field("role_name", &self.role_name)
            .field("expires_at", &self.expires_at)
            .field("connection_string", &"<redacted>")
            .field(
                "connection_string_redacted",
                &self.connection_string_redacted,
            )
            .field("source", &self.source)
            .field("schema_only", &self.schema_only)
            .finish()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteDatabaseOutput {
    pub database_id: String,
    pub database_name: String,
    pub deleted: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStringOutput {
    pub database_id: String,
    pub database_name: String,
    pub expires_at: DateTime<Utc>,
    pub connection_string: String,
    pub connection_string_redacted: String,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunSqlOutput {
    pub database_id: String,
    pub database_name: String,
    pub row_count: Option<u64>,
    pub returned_row_count: usize,
    pub affected_row_count: Option<u64>,
    pub total_row_count_known: bool,
    pub rows: Vec<Value>,
    pub truncated: bool,
    pub elapsed_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DescribeSchemaOutput {
    pub database_id: String,
    pub database_name: String,
    pub relation_counts: SchemaRelationCounts,
    pub tables: Vec<Value>,
    pub columns: Vec<Value>,
    pub constraints: Vec<Value>,
    pub indexes: Vec<Value>,
    pub views: Vec<Value>,
    pub extensions: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupExpiredOutput {
    pub scope: String,
    pub profile: Option<String>,
    pub profiles: Vec<String>,
    pub remaining_profiles: Vec<String>,
    pub dry_run: bool,
    pub selected: Option<Vec<Value>>,
    pub deleted: Option<Vec<String>>,
    pub failures: Option<Vec<Value>>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestOutput {
    pub database_id: String,
    pub database_name: String,
    pub digest_version: u32,
    pub checksum: String,
    pub table_count: usize,
    #[serde(default)]
    pub relation_counts: SchemaRelationCounts,
    pub column_count: usize,
    #[serde(default)]
    pub constraint_count: usize,
    pub index_count: usize,
    pub extension_count: usize,
    pub tables: Vec<SchemaDigestTable>,
    pub extensions: Vec<SchemaDigestExtension>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaRelationCounts {
    pub tables: usize,
    #[serde(default)]
    pub partitioned_tables: usize,
    #[serde(default)]
    pub views: usize,
    #[serde(default)]
    pub materialized_views: usize,
    #[serde(default)]
    pub foreign_tables: usize,
    #[serde(default)]
    pub other: usize,
}

impl SchemaRelationCounts {
    pub fn total(&self) -> usize {
        self.tables
            + self.partitioned_tables
            + self.views
            + self.materialized_views
            + self.foreign_tables
            + self.other
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestTable {
    pub schema: String,
    pub name: String,
    #[serde(default = "default_relation_kind")]
    pub relation_kind: String,
    pub columns: Vec<SchemaDigestColumn>,
    #[serde(default)]
    pub constraints: Vec<SchemaDigestConstraint>,
    pub indexes: Vec<SchemaDigestIndex>,
    #[serde(default)]
    pub view_definition_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    #[serde(default)]
    pub default_expression: Option<String>,
    #[serde(default)]
    pub generated_expression: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestConstraint {
    pub name: String,
    pub constraint_type: String,
    pub definition_hash: String,
    #[serde(default)]
    pub update_action: Option<String>,
    #[serde(default)]
    pub delete_action: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestIndex {
    pub name: String,
    pub definition_hash: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestExtension {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemaDiffBaseDigest {
    Response(SchemaDigestOutput),
    SerializedResponse(String),
}

impl SchemaDiffBaseDigest {
    fn into_schema_digest(self) -> anyhow::Result<SchemaDigestOutput> {
        match self {
            Self::Response(digest) => Ok(digest),
            Self::SerializedResponse(raw) => serde_json::from_str::<SchemaDigestOutput>(&raw)
                .context(
                    "baseDigest string must contain the full JSON schema_digest response, not only the checksum",
                ),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDiffOutput {
    pub database_id: String,
    pub database_name: String,
    pub before_checksum: String,
    pub after_checksum: String,
    pub changed: bool,
    pub added_tables: Vec<String>,
    pub removed_tables: Vec<String>,
    pub changed_tables: Vec<SchemaTableDiff>,
    pub added_extensions: Vec<String>,
    pub removed_extensions: Vec<String>,
    pub changed_extensions: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaTableDiff {
    pub table: String,
    pub added_columns: Vec<String>,
    pub removed_columns: Vec<String>,
    pub changed_columns: Vec<String>,
    pub added_indexes: Vec<String>,
    pub removed_indexes: Vec<String>,
    pub changed_indexes: Vec<String>,
    pub added_constraints: Vec<String>,
    pub removed_constraints: Vec<String>,
    pub changed_constraints: Vec<String>,
    pub view_definition_changed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEnvelope<T: Serialize> {
    pub ok: bool,
    pub summary: String,
    pub changed_objects: Option<SchemaChangeCounts>,
    pub warnings: Vec<String>,
    pub errors: Vec<WorkflowError>,
    pub detail_handles: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_sandbox: Option<T>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowError {
    pub code: String,
    pub category: String,
    pub message: String,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSchemaDigest {
    pub digest_version: u32,
    pub fingerprint: String,
    pub object_counts: SchemaObjectCounts,
    pub tables: Vec<SchemaObjectDigest>,
    pub columns: Vec<SchemaObjectDigest>,
    #[serde(default)]
    pub constraints: Vec<SchemaObjectDigest>,
    pub indexes: Vec<SchemaObjectDigest>,
    pub extensions: Vec<SchemaObjectDigest>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaObjectCounts {
    pub tables: usize,
    #[serde(default)]
    pub partitioned_tables: usize,
    #[serde(default)]
    pub views: usize,
    #[serde(default)]
    pub materialized_views: usize,
    #[serde(default)]
    pub foreign_tables: usize,
    pub columns: usize,
    #[serde(default)]
    pub constraints: usize,
    pub indexes: usize,
    pub extensions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaObjectDigest {
    pub kind: String,
    pub key: String,
    pub fingerprint: String,
    pub summary: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaChangeCounts {
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSchemaDiffOutput {
    pub from_fingerprint: String,
    pub to_fingerprint: String,
    pub changed_objects: SchemaChangeCounts,
    pub added: Vec<WorkflowSchemaDiffItem>,
    pub removed: Vec<WorkflowSchemaDiffItem>,
    pub changed: Vec<WorkflowSchemaDiffChange>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSchemaDiffItem {
    pub kind: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSchemaDiffChange {
    pub kind: String,
    pub key: String,
    pub before_fingerprint: String,
    pub after_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaSnapshotRecord {
    pub snapshot_name: String,
    pub profile: String,
    pub database_id: String,
    pub database_name: String,
    pub owner: Option<String>,
    pub purpose: Option<String>,
    pub labels: Value,
    pub created_at: DateTime<Utc>,
    pub postgres_version: String,
    pub digest_version: u32,
    pub object_counts: SchemaObjectCounts,
    pub notes: Option<String>,
    pub digest: WorkflowSchemaDigest,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainQueryOutput {
    pub database_id: String,
    pub database_name: String,
    pub summary: Value,
    pub plan: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaDigestContent<'a> {
    digest_version: u32,
    tables: &'a [SchemaDigestTable],
    extensions: &'a [SchemaDigestExtension],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaSnapshotSummary {
    pub snapshot_name: String,
    pub profile: String,
    pub database_id: String,
    pub database_name: String,
    pub created_at: DateTime<Utc>,
    pub postgres_version: String,
    pub digest_version: u32,
    pub object_counts: SchemaObjectCounts,
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareForRepoOutput {
    pub repo_path: String,
    pub postgres_version: Option<String>,
    pub postgres_version_source: Option<String>,
    pub config_path: Option<String>,
    pub sandbox_target: Option<String>,
    pub migration_command_configured: bool,
    pub seed_command_configured: bool,
    pub action_needed: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandWorkflowOutput {
    pub database_id: String,
    pub database_name: String,
    pub command: Vec<String>,
    pub elapsed_ms: u128,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateSchemaChangeOutput {
    pub database_id: String,
    pub database_name: String,
    pub created_sandbox: bool,
    pub command: Vec<String>,
    pub elapsed_ms: u128,
    pub exit_code: Option<i32>,
    pub schema_diff: WorkflowSchemaDiffOutput,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateMetadata {
    pub template_name: String,
    pub profile: String,
    pub source_sandbox_id: String,
    pub source_database_name: String,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<String>,
    pub owner: Option<String>,
    pub postgres_version: String,
    pub size_bytes: u64,
    pub notes: Option<String>,
    pub privacy_warning: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTemplateOutput {
    pub metadata: TemplateMetadata,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSandboxFromTemplateOutput {
    pub database_id: String,
    pub profile: String,
    pub database_name: String,
    pub role_name: String,
    pub expires_at: DateTime<Utc>,
    #[serde(skip_serializing)]
    pub connection_string: String,
    pub connection_string_redacted: String,
    pub template_name: String,
}

impl fmt::Debug for CreateSandboxFromTemplateOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreateSandboxFromTemplateOutput")
            .field("database_id", &self.database_id)
            .field("profile", &self.profile)
            .field("database_name", &self.database_name)
            .field("role_name", &self.role_name)
            .field("expires_at", &self.expires_at)
            .field("connection_string", &"<redacted>")
            .field(
                "connection_string_redacted",
                &self.connection_string_redacted,
            )
            .field("template_name", &self.template_name)
            .finish()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteTemplateOutput {
    pub template_name: String,
    pub deleted: bool,
}

#[derive(Debug)]
struct SandboxRecord {
    database_id: String,
    profile_name: String,
    database_name: String,
    role_name: String,
    role_password: String,
    owner: Option<String>,
    purpose: Option<String>,
    labels: Value,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

struct QueryExecutionResult {
    row_count: Option<u64>,
    returned_row_count: usize,
    affected_row_count: Option<u64>,
    total_row_count_known: bool,
    rows: Vec<Value>,
    truncated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoProjectConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    migration_command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seed_command: Option<Vec<String>>,
    #[serde(default = "default_database_url_env")]
    database_url_env: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    postgres_version: Option<String>,
    prepared_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoPostgresVersionInference {
    version: String,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoPostgresVersionResolution {
    version: Option<String>,
    source: Option<String>,
}

#[derive(Debug)]
struct CommandRunResult {
    command: Vec<String>,
    elapsed_ms: u128,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

#[derive(Debug)]
struct TemplatePaths {
    dump_path: PathBuf,
    metadata_path: PathBuf,
}

#[derive(Debug)]
struct SnapshotPaths {
    metadata_path: PathBuf,
}

#[derive(Clone, Copy)]
enum QueryMode {
    Cursor,
    TypedRows,
    Simple,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArrayCellKind {
    Text,
    Bool,
    Int2,
    Int4,
    Int8,
    Float4,
    Float8,
    Json,
    Date,
    Timestamp,
    TimestampTz,
    Uuid,
}

#[derive(Debug)]
struct PgNumeric(String);

#[derive(Debug)]
struct PgTimeTz(String);

impl<'a> FromSql<'a> for PgNumeric {
    fn from_sql(
        ty: &Type,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        if *ty != Type::NUMERIC {
            return Err(format!("unsupported type for PgNumeric: {}", ty.name()).into());
        }
        Ok(Self(decode_pg_numeric(raw)?))
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::NUMERIC
    }
}

impl<'a> FromSql<'a> for PgTimeTz {
    fn from_sql(
        ty: &Type,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        if *ty != Type::TIMETZ {
            return Err(format!("unsupported type for PgTimeTz: {}", ty.name()).into());
        }
        Ok(Self(decode_pg_timetz(raw)?))
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::TIMETZ
    }
}

impl PostgresSandboxManager {
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    fn resolve_profile(
        &self,
        profile_name: Option<&str>,
        postgres_version: Option<&str>,
    ) -> anyhow::Result<SandboxProfile> {
        match find_profile_for_request(&self.config, profile_name, postgres_version) {
            Ok(profile) if profile.managed_local => {
                let version = postgres_version.or(profile.postgres_version.as_deref());
                self.ensure_managed_local_profile(version)
            }
            Ok(profile) => Ok(profile.clone()),
            Err(ConfigError::UnknownPostgresVersion(version))
                if self.config.managed_local.enabled && profile_name.is_none() =>
            {
                self.ensure_managed_local_profile(Some(&version))
            }
            Err(ConfigError::UnknownPostgresVersion(version)) => {
                Err(unknown_postgres_version_error(&self.config, &version))
            }
            Err(ConfigError::UnknownProfile(profile))
                if self.config.managed_local.enabled
                    && postgres_version.is_none()
                    && profile_name == Some(profile.as_str()) =>
            {
                let Some(version) = profile.strip_prefix("local-pg") else {
                    return Err(ConfigError::UnknownProfile(profile).into());
                };
                self.ensure_managed_local_profile(Some(version))
            }
            Err(error) => Err(error.into()),
        }
    }

    fn ensure_managed_local_profile(
        &self,
        postgres_version: Option<&str>,
    ) -> anyhow::Result<SandboxProfile> {
        let local_config = LocalPostgresCluster::from_env_for_version(postgres_version)?
            .ensure_started()
            .with_context(|| match postgres_version {
                Some(version) => {
                    format!(
                        "failed to prepare local Postgres profile for postgresVersion {version}"
                    )
                }
                None => "failed to prepare default local Postgres profile".to_string(),
            })?;
        Ok(self.profile_from_local_config(local_config))
    }

    fn profile_from_local_config(&self, local_config: LocalClusterConfig) -> SandboxProfile {
        let base = self
            .config
            .profiles
            .iter()
            .find(|profile| profile.managed_local)
            .or_else(|| find_profile(&self.config, None).ok());
        SandboxProfile {
            name: local_config.profile_name,
            admin_url: local_config.admin_url,
            database_prefix: base
                .map(|profile| profile.database_prefix.clone())
                .unwrap_or_else(|| "pgsandbox".to_string()),
            default_ttl_minutes: base.map_or(240, |profile| profile.default_ttl_minutes),
            max_ttl_minutes: base.map_or(1440, |profile| profile.max_ttl_minutes),
            allow_external_admin_url: false,
            allowed_admin_hosts: Vec::new(),
            max_active_databases_per_owner: base
                .and_then(|profile| profile.max_active_databases_per_owner),
            postgres_version: local_config.postgres_version,
            managed_local: true,
        }
    }

    pub fn list_profiles(&self, input: ListProfilesInput) -> anyhow::Result<ListProfilesOutput> {
        let mut profiles = self
            .config
            .profiles
            .iter()
            .map(|profile| ProfileSummary {
                name: profile.name.clone(),
                default: profile.name == self.config.default_profile,
                postgres_version: profile.postgres_version.clone(),
                port: profile_admin_url_port(profile),
                managed_local: profile.managed_local,
                admin_url: profile_admin_url_summary(profile),
                source: "configured".to_string(),
            })
            .collect::<Vec<_>>();
        let mut available_postgres_versions = profiles
            .iter()
            .filter(|profile| profile.managed_local)
            .filter_map(|profile| profile.postgres_version.clone())
            .collect::<Vec<_>>();

        if self.config.managed_local.enabled && input.include_discovered_local.unwrap_or(true) {
            for installation in discover_local_postgres_installations() {
                if !available_postgres_versions
                    .iter()
                    .any(|version| version == &installation.postgres_version)
                {
                    available_postgres_versions.push(installation.postgres_version.clone());
                }
                if profiles.iter().any(|profile| {
                    profile.managed_local
                        && profile.postgres_version.as_deref()
                            == Some(installation.postgres_version.as_str())
                }) {
                    continue;
                }
                let name = format!("local-pg{}", installation.postgres_version);
                profiles.push(ProfileSummary {
                    name,
                    default: false,
                    postgres_version: Some(installation.postgres_version.clone()),
                    port: None,
                    managed_local: true,
                    admin_url: "(managed local; starts on demand)".to_string(),
                    source: installation.source,
                });
            }
        }
        available_postgres_versions.sort();
        available_postgres_versions.dedup();

        Ok(ListProfilesOutput {
            server_version: crate::VERSION.to_string(),
            tool_count: PUBLIC_MCP_TOOL_COUNT,
            restart_required_after_setup_note: restart_required_after_setup_note(),
            available_postgres_versions,
            hints: list_profile_hints(&self.config),
            profiles,
        })
    }

    async fn get_owned_record(
        &self,
        profile_name: Option<String>,
        postgres_version: Option<String>,
        database_id: Option<String>,
        database_name: Option<String>,
    ) -> anyhow::Result<(SandboxProfile, SandboxRecord)> {
        if selector_is_unscoped_database_id(
            profile_name.as_ref(),
            postgres_version.as_ref(),
            database_id.as_ref(),
            database_name.as_ref(),
        ) {
            return self
                .get_owned_record_by_database_id(database_id.unwrap_or_default())
                .await;
        }
        if selector_is_unscoped_database_name(
            profile_name.as_ref(),
            postgres_version.as_ref(),
            database_id.as_ref(),
            database_name.as_ref(),
        ) {
            return self
                .get_owned_record_by_database_name(database_name.unwrap_or_default())
                .await;
        }

        let profile = self.resolve_profile(profile_name.as_deref(), postgres_version.as_deref())?;
        let (client, connection_task) = connect_admin(&profile).await?;
        ensure_metadata_table(&client).await?;
        let selector = DatabaseSelector {
            profile: None,
            postgres_version: None,
            database_id,
            database_name,
        };
        let record = find_record(&client, &profile.name, &selector)
            .await?
            .context("Database was not found in PGSandbox metadata.")?;
        drop(client);
        let _ = connection_task.await;
        Ok((profile, record))
    }

    async fn get_owned_record_by_database_id(
        &self,
        database_id: String,
    ) -> anyhow::Result<(SandboxProfile, SandboxRecord)> {
        self.get_owned_record_across_profiles("databaseId", database_id, |selector, value| {
            selector.database_id = Some(value.to_string())
        })
        .await
    }

    async fn get_owned_record_by_database_name(
        &self,
        database_name: String,
    ) -> anyhow::Result<(SandboxProfile, SandboxRecord)> {
        self.get_owned_record_across_profiles("databaseName", database_name, |selector, value| {
            selector.database_name = Some(value.to_string())
        })
        .await
    }

    async fn get_owned_record_across_profiles(
        &self,
        selector_label: &'static str,
        selector_value: String,
        apply_selector: impl Fn(&mut DatabaseSelector, &str) + Copy,
    ) -> anyhow::Result<(SandboxProfile, SandboxRecord)> {
        let mut matches = Vec::new();
        let mut search_errors = Vec::new();

        for profile in self.profiles_for_all_version_operations()? {
            let profile_name = profile.name.clone();
            let search = async {
                let (client, connection_task) = connect_admin(&profile).await?;
                ensure_metadata_table(&client).await?;
                let mut selector = DatabaseSelector {
                    profile: None,
                    postgres_version: None,
                    database_id: None,
                    database_name: None,
                };
                apply_selector(&mut selector, &selector_value);
                let record = find_record(&client, &profile.name, &selector).await?;
                drop(client);
                let _ = connection_task.await;
                anyhow::Ok(record)
            }
            .await;

            match search {
                Ok(Some(record)) => matches.push((profile, record)),
                Ok(None) => {}
                Err(error) => search_errors.push(format!("{profile_name}: {error:#}")),
            }
        }

        match matches.len() {
            1 => Ok(matches.remove(0)),
            0 => {
                let mut message = format!(
                    "Database was not found in PGSandbox metadata for {selector_label} {selector_value}. If this sandbox was created under a specific profile or postgresVersion, retry with that profile or postgresVersion, or call list_databases with includeAllVersions=true."
                );
                if !search_errors.is_empty() {
                    let summarized = search_errors
                        .into_iter()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join("; ");
                    message.push_str(" Some profiles could not be searched: ");
                    message.push_str(&summarized);
                }
                anyhow::bail!(message)
            }
            _ => {
                let profiles = matches
                    .iter()
                    .map(|(profile, _)| profile.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!(
                    "{selector_label} {selector_value} matched multiple PGSandbox profiles ({profiles}); retry with profile or postgresVersion."
                )
            }
        }
    }

    pub async fn create_database(
        &self,
        input: CreateDatabaseInput,
    ) -> anyhow::Result<CreateDatabaseOutput> {
        let profile =
            self.resolve_profile(input.profile.as_deref(), input.postgres_version.as_deref())?;
        let ttl_minutes = clamp_ttl(input.ttl_minutes, &profile)?;
        let names = make_sandbox_names(&profile.database_prefix, input.name_hint.as_deref());
        let role_password = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let expires_at = Utc::now() + Duration::minutes(ttl_minutes.into());

        let (client, connection_task) = connect_admin(&profile).await?;
        ensure_metadata_table(&client).await?;
        enforce_owner_quota(&client, &profile, input.owner.as_deref()).await?;

        let mut created_role = false;
        let mut created_database = false;
        let result = async {
            client
                .batch_execute(&format!(
                    "CREATE ROLE {} LOGIN PASSWORD {}",
                    quote_ident(&names.role_name)?,
                    quote_literal(&role_password)
                ))
                .await?;
            created_role = true;

            client
                .batch_execute(&format!(
                    "CREATE DATABASE {} OWNER {}",
                    quote_ident(&names.database_name)?,
                    quote_ident(&names.role_name)?
                ))
                .await?;
            created_database = true;

            let labels = serde_json::to_value(input.labels.unwrap_or_default())?;
            let stored_role_password = protect_role_password(&role_password, &profile)?;
            client
                .execute(
                    &format!(
                        r#"
                          INSERT INTO {}
                            (database_id, profile_name, database_name, role_name, role_password, owner, purpose, labels, expires_at)
                          VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9)
                        "#,
                        quote_ident(METADATA_TABLE)?
                    ),
                    &[
                        &names.database_id as &(dyn ToSql + Sync),
                        &profile.name,
                        &names.database_name,
                        &names.role_name,
                        &stored_role_password,
                        &input.owner,
                        &input.name_hint,
                        &labels,
                        &expires_at,
                    ],
                )
                .await?;
            let _ = record_audit_event(
                &client,
                "create_database",
                &profile.name,
                &names.database_id,
                &names.database_name,
                Some(&names.role_name),
                json!({
                    "owner": input.owner,
                    "purpose": input.name_hint,
                    "expiresAt": expires_at,
                }),
            )
            .await;
            anyhow::Ok(())
        }
        .await;

        if let Err(error) = result {
            if created_database {
                let _ = terminate_database_connections(&client, &names.database_name).await;
                let _ = client
                    .batch_execute(&format!(
                        "DROP DATABASE IF EXISTS {}",
                        quote_ident(&names.database_name)?
                    ))
                    .await;
            }
            if created_role {
                let _ = client
                    .batch_execute(&format!(
                        "DROP ROLE IF EXISTS {}",
                        quote_ident(&names.role_name)?
                    ))
                    .await;
            }
            let _ = client
                .execute(
                    &format!(
                        "UPDATE {} SET deleted_at = now() WHERE database_id = $1",
                        quote_ident(METADATA_TABLE)?
                    ),
                    &[&names.database_id],
                )
                .await;
            let _ = record_audit_event(
                &client,
                "create_database_rolled_back",
                &profile.name,
                &names.database_id,
                &names.database_name,
                Some(&names.role_name),
                json!({ "message": error.to_string() }),
            )
            .await;
            drop(client);
            let _ = connection_task.await;
            return Err(error);
        }

        drop(client);
        let _ = connection_task.await;

        let connection_string = build_connection_string(
            &profile.admin_url,
            &names.database_name,
            &names.role_name,
            &role_password,
        )?;

        Ok(CreateDatabaseOutput {
            database_id: names.database_id,
            profile: profile.name.clone(),
            database_name: names.database_name.clone(),
            role_name: names.role_name.clone(),
            expires_at,
            connection_string_redacted: mask_connection_string(&connection_string),
            connection_string,
        })
    }

    pub async fn clone_database(
        &self,
        input: CloneDatabaseInput,
    ) -> anyhow::Result<CloneDatabaseOutput> {
        let CloneDatabaseInput {
            profile,
            postgres_version,
            source_database_url,
            name_hint,
            ttl_minutes,
            owner,
            labels,
            schema_only,
        } = input;
        let schema_only = schema_only.unwrap_or(false);
        let target_profile =
            self.resolve_profile(profile.as_deref(), postgres_version.as_deref())?;
        preflight_clone_compatibility(&source_database_url, &target_profile).await?;
        let created = self
            .create_database(CreateDatabaseInput {
                profile: Some(target_profile.name.clone()),
                postgres_version: None,
                name_hint,
                ttl_minutes,
                owner,
                labels,
            })
            .await?;

        let clone_result = clone_with_pg_tools(
            &source_database_url,
            &created.connection_string,
            schema_only,
        )
        .await;

        if let Err(error) = clone_result {
            let cleanup_result = self
                .delete_database(DatabaseSelector {
                    profile: Some(created.profile.clone()),
                    postgres_version: None,
                    database_id: Some(created.database_id.clone()),
                    database_name: None,
                })
                .await;
            match cleanup_result {
                Ok(_) => {
                    anyhow::bail!("database clone failed; created sandbox was deleted: {error}")
                }
                Err(cleanup_error) => anyhow::bail!(
                    "database clone failed and cleanup also failed for {}: {error}; cleanup error: {cleanup_error}",
                    created.database_name
                ),
            }
        }

        Ok(CloneDatabaseOutput {
            database_id: created.database_id,
            profile: created.profile,
            database_name: created.database_name,
            role_name: created.role_name,
            expires_at: created.expires_at,
            connection_string_redacted: mask_connection_string(&created.connection_string),
            connection_string: created.connection_string,
            source: "external".to_string(),
            schema_only,
        })
    }

    pub async fn delete_database(
        &self,
        input: DatabaseSelector,
    ) -> anyhow::Result<DeleteDatabaseOutput> {
        let (profile, record) = self
            .get_owned_record(
                input.profile,
                input.postgres_version,
                input.database_id,
                input.database_name,
            )
            .await?;
        let (client, connection_task) = connect_admin(&profile).await?;

        terminate_database_connections(&client, &record.database_name).await?;
        client
            .batch_execute(&format!(
                "DROP DATABASE IF EXISTS {}",
                quote_ident(&record.database_name)?
            ))
            .await?;
        client
            .batch_execute(&format!(
                "DROP ROLE IF EXISTS {}",
                quote_ident(&record.role_name)?
            ))
            .await?;
        client
            .execute(
                &format!(
                    "UPDATE {} SET deleted_at = now() WHERE database_id = $1",
                    quote_ident(METADATA_TABLE)?
                ),
                &[&record.database_id],
            )
            .await?;
        let _ = record_audit_event(
            &client,
            "delete_database",
            &profile.name,
            &record.database_id,
            &record.database_name,
            Some(&record.role_name),
            json!({ "deleted": true }),
        )
        .await;

        drop(client);
        let _ = connection_task.await;

        Ok(DeleteDatabaseOutput {
            database_id: record.database_id,
            database_name: record.database_name,
            deleted: true,
        })
    }

    pub async fn get_connection_string(
        &self,
        input: DatabaseSelector,
    ) -> anyhow::Result<ConnectionStringOutput> {
        let (profile, record) = self
            .get_owned_record(
                input.profile,
                input.postgres_version,
                input.database_id,
                input.database_name,
            )
            .await?;

        let connection_string = build_connection_string(
            &profile.admin_url,
            &record.database_name,
            &record.role_name,
            &unprotect_role_password(&record.role_password, &profile)?,
        )?;

        Ok(ConnectionStringOutput {
            database_id: record.database_id,
            database_name: record.database_name.clone(),
            expires_at: record.expires_at,
            connection_string_redacted: mask_connection_string(&connection_string),
            connection_string,
        })
    }

    pub async fn list_databases(
        &self,
        input: ListDatabasesInput,
    ) -> anyhow::Result<ListDatabasesOutput> {
        if all_versions_requested(
            input.postgres_version.as_deref(),
            input.include_all_versions,
        ) {
            if input.profile.is_some() {
                anyhow::bail!(
                    "includeAllVersions/postgresVersion=\"*\" cannot be combined with profile; omit profile to list configured profiles and running managed-local versions."
                );
            }
            let profiles = self.profiles_for_all_version_operations()?;
            let profile_names = profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect::<Vec<_>>();
            let mut databases = Vec::new();
            let mut failures = Vec::new();
            let mut truncated = false;
            for profile in &profiles {
                match self
                    .list_databases_for_profile(profile, input.owner.as_ref())
                    .await
                {
                    Ok(result) => {
                        truncated |= result.truncated;
                        databases.extend(result.databases);
                    }
                    Err(error) => failures.push(profile_operation_failure(profile, error)),
                }
            }
            if databases.len() > LIST_DATABASES_LIMIT {
                databases.truncate(LIST_DATABASES_LIMIT);
                truncated = true;
            }
            return Ok(ListDatabasesOutput {
                scope: "allVersions".to_string(),
                profiles: profile_names,
                databases,
                truncated,
                failures,
            });
        }

        let profile =
            self.resolve_profile(input.profile.as_deref(), input.postgres_version.as_deref())?;
        self.list_databases_for_profile(&profile, input.owner.as_ref())
            .await
    }

    async fn list_databases_for_profile(
        &self,
        profile: &SandboxProfile,
        owner: Option<&String>,
    ) -> anyhow::Result<ListDatabasesOutput> {
        let (client, connection_task) = connect_admin(profile).await?;
        ensure_metadata_table(&client).await?;
        let owner = owner.map(String::as_str);
        let rows = client
            .query(
                &format!(
                    r#"
                      SELECT database_id, profile_name, database_name, role_name, owner, purpose, labels,
                             created_at, expires_at, deleted_at
                      FROM {}
                      WHERE profile_name = $1
                        AND deleted_at IS NULL
                        AND ($2::text IS NULL OR owner = $2)
                      ORDER BY created_at DESC
                      LIMIT {}
                    "#,
                    quote_ident(METADATA_TABLE)?,
                    LIST_DATABASES_LIMIT + 1
                ),
                &[&profile.name, &owner],
            )
            .await?;
        drop(client);
        let _ = connection_task.await;

        let truncated = rows.len() > LIST_DATABASES_LIMIT;
        Ok(ListDatabasesOutput {
            scope: "profile".to_string(),
            profiles: vec![profile.name.clone()],
            databases: rows
                .iter()
                .take(LIST_DATABASES_LIMIT)
                .map(record_summary_to_json)
                .collect(),
            truncated,
            failures: Vec::new(),
        })
    }

    pub async fn run_sql(&self, input: RunSqlInput) -> anyhow::Result<RunSqlOutput> {
        let connection = self
            .get_connection_string(DatabaseSelector {
                profile: input.profile.clone(),
                postgres_version: input.postgres_version.clone(),
                database_id: input.database_id.clone(),
                database_name: input.database_name.clone(),
            })
            .await?;
        let started = std::time::Instant::now();
        let row_limit = input.row_limit.unwrap_or(DEFAULT_ROW_LIMIT).min(1000);
        let (client, connection_task) = connect_url(&connection.connection_string).await?;

        let result = if input.readonly.unwrap_or(false) {
            run_readonly_query(&client, &input.sql, row_limit).await
        } else {
            run_sql_body(&client, &input.sql, row_limit, true).await
        };

        drop(client);
        let _ = connection_task.await;

        let result = result?;
        Ok(RunSqlOutput {
            database_id: connection.database_id,
            database_name: connection.database_name,
            row_count: result.row_count,
            returned_row_count: result.returned_row_count,
            affected_row_count: result.affected_row_count,
            total_row_count_known: result.total_row_count_known,
            rows: result.rows,
            truncated: result.truncated,
            elapsed_ms: started.elapsed().as_millis(),
        })
    }

    pub async fn describe_schema(
        &self,
        input: DatabaseSelector,
    ) -> anyhow::Result<DescribeSchemaOutput> {
        let connection = self.get_connection_string(input).await?;
        let (client, connection_task) = connect_url(&connection.connection_string).await?;

        let tables = client
            .query(
                r#"
                  SELECT n.nspname AS table_schema,
                         c.relname AS table_name,
                         CASE c.relkind
                           WHEN 'r' THEN 'table'
                           WHEN 'p' THEN 'partitioned_table'
                           WHEN 'v' THEN 'view'
                           WHEN 'm' THEN 'materialized_view'
                           WHEN 'f' THEN 'foreign_table'
                           ELSE c.relkind::text
                         END AS relation_kind,
                         n.nspname AS "tableSchema",
                         c.relname AS "tableName",
                         CASE c.relkind
                           WHEN 'r' THEN 'table'
                           WHEN 'p' THEN 'partitioned_table'
                           WHEN 'v' THEN 'view'
                           WHEN 'm' THEN 'materialized_view'
                           WHEN 'f' THEN 'foreign_table'
                           ELSE c.relkind::text
                         END AS "relationKind"
                  FROM pg_class c
                  JOIN pg_namespace n ON n.oid = c.relnamespace
                  WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f')
                    AND n.nspname NOT IN ('pg_catalog', 'information_schema')
                  ORDER BY n.nspname, c.relname
                "#,
                &[],
            )
            .await?;
        let columns = client
            .query(
                r#"
                  SELECT n.nspname AS table_schema,
                         c.relname AS table_name,
                         a.attname AS column_name,
                         pg_catalog.format_type(a.atttypid, a.atttypmod) AS data_type,
                         CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END AS is_nullable,
                         CASE WHEN a.attgenerated = '' THEN pg_get_expr(ad.adbin, ad.adrelid) ELSE NULL END AS column_default,
                         CASE WHEN a.attgenerated = '' THEN NULL ELSE a.attgenerated::text END AS generated_kind,
                         CASE WHEN a.attgenerated = '' THEN NULL ELSE pg_get_expr(ad.adbin, ad.adrelid) END AS generation_expression,
                         n.nspname AS "tableSchema",
                         c.relname AS "tableName",
                         a.attname AS "columnName",
                         pg_catalog.format_type(a.atttypid, a.atttypmod) AS "dataType",
                         CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END AS "isNullable",
                         CASE WHEN a.attgenerated = '' THEN pg_get_expr(ad.adbin, ad.adrelid) ELSE NULL END AS "columnDefault",
                         CASE WHEN a.attgenerated = '' THEN NULL ELSE a.attgenerated::text END AS "generatedKind",
                         CASE WHEN a.attgenerated = '' THEN NULL ELSE pg_get_expr(ad.adbin, ad.adrelid) END AS "generationExpression"
                  FROM pg_attribute a
                  JOIN pg_class c ON c.oid = a.attrelid
                  JOIN pg_namespace n ON n.oid = c.relnamespace
                  LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum
                  WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f')
                    AND a.attnum > 0
                    AND NOT a.attisdropped
                    AND n.nspname NOT IN ('pg_catalog', 'information_schema')
                  ORDER BY n.nspname, c.relname, a.attnum
                "#,
                &[],
            )
            .await?;
        let constraints = client
            .query(
                r#"
                  SELECT n.nspname AS table_schema,
                         c.relname AS table_name,
                         con.conname AS constraint_name,
                         CASE con.contype
                           WHEN 'p' THEN 'primary_key'
                           WHEN 'u' THEN 'unique'
                           WHEN 'f' THEN 'foreign_key'
                           WHEN 'c' THEN 'check'
                           WHEN 'x' THEN 'exclusion'
                           ELSE con.contype::text
                         END AS constraint_type,
                         pg_get_constraintdef(con.oid, true) AS definition,
                         CASE con.confupdtype
                           WHEN 'a' THEN 'no_action'
                           WHEN 'r' THEN 'restrict'
                           WHEN 'c' THEN 'cascade'
                           WHEN 'n' THEN 'set_null'
                           WHEN 'd' THEN 'set_default'
                           ELSE NULL
                         END AS update_action,
                         CASE con.confdeltype
                           WHEN 'a' THEN 'no_action'
                           WHEN 'r' THEN 'restrict'
                           WHEN 'c' THEN 'cascade'
                           WHEN 'n' THEN 'set_null'
                           WHEN 'd' THEN 'set_default'
                           ELSE NULL
                         END AS delete_action,
                         n.nspname AS "tableSchema",
                         c.relname AS "tableName",
                         con.conname AS "constraintName",
                         CASE con.contype
                           WHEN 'p' THEN 'primary_key'
                           WHEN 'u' THEN 'unique'
                           WHEN 'f' THEN 'foreign_key'
                           WHEN 'c' THEN 'check'
                           WHEN 'x' THEN 'exclusion'
                           ELSE con.contype::text
                         END AS "constraintType",
                         pg_get_constraintdef(con.oid, true) AS "definition",
                         CASE con.confupdtype
                           WHEN 'a' THEN 'no_action'
                           WHEN 'r' THEN 'restrict'
                           WHEN 'c' THEN 'cascade'
                           WHEN 'n' THEN 'set_null'
                           WHEN 'd' THEN 'set_default'
                           ELSE NULL
                         END AS "updateAction",
                         CASE con.confdeltype
                           WHEN 'a' THEN 'no_action'
                           WHEN 'r' THEN 'restrict'
                           WHEN 'c' THEN 'cascade'
                           WHEN 'n' THEN 'set_null'
                           WHEN 'd' THEN 'set_default'
                           ELSE NULL
                         END AS "deleteAction"
                  FROM pg_constraint con
                  JOIN pg_class c ON c.oid = con.conrelid
                  JOIN pg_namespace n ON n.oid = c.relnamespace
                  WHERE c.relkind IN ('r', 'p')
                    AND n.nspname NOT IN ('pg_catalog', 'information_schema')
                  ORDER BY n.nspname, c.relname, con.conname
                "#,
                &[],
            )
            .await?;
        let indexes = client
            .query(
                r#"
                  SELECT schemaname,
                         tablename,
                         indexname,
                         indexdef,
                         schemaname AS "schemaName",
                         tablename AS "tableName",
                         indexname AS "indexName",
                         indexdef AS "definition"
                  FROM pg_indexes
                  WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
                  ORDER BY schemaname, tablename, indexname
                "#,
                &[],
            )
            .await?;
        let views = client
            .query(
                r#"
                  SELECT n.nspname AS table_schema,
                         c.relname AS table_name,
                         CASE c.relkind
                           WHEN 'v' THEN 'view'
                           WHEN 'm' THEN 'materialized_view'
                         END AS relation_kind,
                         pg_get_viewdef(c.oid, true) AS definition,
                         n.nspname AS "tableSchema",
                         c.relname AS "tableName",
                         CASE c.relkind
                           WHEN 'v' THEN 'view'
                           WHEN 'm' THEN 'materialized_view'
                         END AS "relationKind",
                         pg_get_viewdef(c.oid, true) AS "definition"
                  FROM pg_class c
                  JOIN pg_namespace n ON n.oid = c.relnamespace
                  WHERE c.relkind IN ('v', 'm')
                    AND n.nspname NOT IN ('pg_catalog', 'information_schema')
                  ORDER BY n.nspname, c.relname
                "#,
                &[],
            )
            .await?;
        let extensions = client
            .query(
                r#"
                  SELECT extname,
                         extversion,
                         extname AS "name",
                         extversion AS "version"
                  FROM pg_extension
                  ORDER BY extname
                "#,
                &[],
            )
            .await?;

        let relation_counts = relation_counts_from_rows(&tables);
        drop(client);
        let _ = connection_task.await;

        Ok(DescribeSchemaOutput {
            database_id: connection.database_id,
            database_name: connection.database_name,
            relation_counts,
            tables: rows_to_json(tables)?,
            columns: rows_to_json(columns)?,
            constraints: rows_to_json(constraints)?,
            indexes: rows_to_json(indexes)?,
            views: rows_to_json(views)?,
            extensions: rows_to_json(extensions)?,
        })
    }

    pub async fn schema_digest(
        &self,
        input: DatabaseSelector,
    ) -> anyhow::Result<SchemaDigestOutput> {
        let connection = self.get_connection_string(input).await?;
        let (client, connection_task) = connect_url(&connection.connection_string).await?;

        let digest = schema_digest_for_connection(
            &client,
            connection.database_id.clone(),
            connection.database_name.clone(),
        )
        .await;

        drop(client);
        let _ = connection_task.await;

        digest
    }

    pub async fn schema_diff(&self, input: SchemaDiffInput) -> anyhow::Result<SchemaDiffOutput> {
        let before = input
            .base_digest
            .into_schema_digest()
            .context("baseDigest must be a schema_digest response")?;
        let after = self
            .schema_digest(DatabaseSelector {
                profile: input.profile,
                postgres_version: input.postgres_version,
                database_id: input.database_id,
                database_name: input.database_name,
            })
            .await?;

        diff_schema_digests(&before, &after)
    }

    pub async fn explain_query(
        &self,
        input: ExplainQueryInput,
    ) -> anyhow::Result<ExplainQueryOutput> {
        let connection = self
            .get_connection_string(DatabaseSelector {
                profile: input.profile,
                postgres_version: input.postgres_version,
                database_id: input.database_id,
                database_name: input.database_name,
            })
            .await?;
        let statement = explainable_statement(&input.sql)?;
        let explain_sql = format!("EXPLAIN (FORMAT JSON) {statement}");
        let (client, connection_task) = connect_url(&connection.connection_string).await?;

        let result = async {
            let row = client.query_one(&explain_sql, &[]).await?;
            let plan = row
                .try_get::<_, Value>(0)
                .context("Postgres did not return JSON explain output")?;
            let summary = explain_summary(&plan);
            anyhow::Ok(ExplainQueryOutput {
                database_id: connection.database_id,
                database_name: connection.database_name,
                summary,
                plan,
            })
        }
        .await;

        drop(client);
        let _ = connection_task.await;

        result
    }

    pub async fn create_schema_snapshot(
        &self,
        input: CreateSchemaSnapshotInput,
    ) -> anyhow::Result<WorkflowEnvelope<SchemaSnapshotSummary>> {
        let snapshot_name = match validate_artifact_name(&input.snapshot_name, "snapshotName") {
            Ok(value) => value,
            Err(error) => {
                return Ok(workflow_failure(
                    "Schema snapshot was not created.",
                    error,
                    None,
                ))
            }
        };
        let (profile, record) = self
            .get_owned_record(
                input.profile,
                input.postgres_version,
                input.database_id,
                input.database_name,
            )
            .await?;
        let connection_string = build_connection_string(
            &profile.admin_url,
            &record.database_name,
            &record.role_name,
            &unprotect_role_password(&record.role_password, &profile)?,
        )?;
        let (postgres_version, digest) =
            match collect_schema_snapshot_digest_for_url(&connection_string).await {
                Ok(result) => result,
                Err(error) => {
                    return Ok(workflow_failure(
                        "Schema snapshot was not created.",
                        schema_snapshot_workflow_error(error),
                        None,
                    ))
                }
            };

        let snapshot = SchemaSnapshotRecord {
            snapshot_name: snapshot_name.clone(),
            profile: profile.name.clone(),
            database_id: record.database_id.clone(),
            database_name: record.database_name.clone(),
            owner: record.owner.clone(),
            purpose: record.purpose.clone(),
            labels: record.labels.clone(),
            created_at: Utc::now(),
            postgres_version,
            digest_version: digest.digest_version,
            object_counts: digest.object_counts.clone(),
            notes: input.notes,
            digest,
        };
        let paths = snapshot_paths(&profile.name, &record.database_id, &snapshot_name)?;
        if let Err(error) = write_json_file(&paths.metadata_path, &snapshot) {
            return Ok(workflow_failure(
                "Schema snapshot was not created.",
                workflow_error(
                    "schema_snapshot_failed",
                    error.to_string(),
                    Some(
                        "Check that PGSANDBOX_HOME is writable and retry snapshot creation."
                            .to_string(),
                    ),
                ),
                None,
            ));
        }
        let summary = snapshot_summary(&snapshot);

        Ok(workflow_success(
            format!("Schema snapshot `{snapshot_name}` created."),
            Some(SchemaChangeCounts::default()),
            Vec::new(),
            vec![snapshot_detail_handle(
                &profile.name,
                &record.database_id,
                &snapshot_name,
            )],
            summary,
        ))
    }

    pub async fn list_schema_snapshots(
        &self,
        input: ListSchemaSnapshotsInput,
    ) -> anyhow::Result<WorkflowEnvelope<Vec<SchemaSnapshotSummary>>> {
        let (profile, record) = self
            .get_owned_record(
                input.profile,
                input.postgres_version,
                input.database_id,
                input.database_name,
            )
            .await?;
        let snapshots = read_schema_snapshots(&profile.name, &record.database_id)?;

        Ok(workflow_success(
            format!("Found {} schema snapshot(s).", snapshots.len()),
            None,
            Vec::new(),
            vec![json!({
                "type": "schema-snapshot-list",
                "profile": profile.name,
                "databaseId": record.database_id
            })],
            snapshots
                .into_iter()
                .map(|snapshot| snapshot_summary(&snapshot))
                .collect(),
        ))
    }

    pub async fn delete_schema_snapshot(
        &self,
        input: DeleteSchemaSnapshotInput,
    ) -> anyhow::Result<WorkflowEnvelope<Value>> {
        let snapshot_name = match validate_artifact_name(&input.snapshot_name, "snapshotName") {
            Ok(value) => value,
            Err(error) => {
                return Ok(workflow_failure(
                    "Schema snapshot was not deleted.",
                    error,
                    None,
                ))
            }
        };
        let (profile, record) = self
            .get_owned_record(
                input.profile,
                input.postgres_version,
                input.database_id,
                input.database_name,
            )
            .await?;
        let paths = snapshot_paths(&profile.name, &record.database_id, &snapshot_name)?;
        let deleted = remove_file_if_exists(&paths.metadata_path)?;

        Ok(workflow_success(
            if deleted {
                format!("Schema snapshot `{snapshot_name}` deleted.")
            } else {
                format!("Schema snapshot `{snapshot_name}` did not exist.")
            },
            None,
            Vec::new(),
            vec![snapshot_detail_handle(
                &profile.name,
                &record.database_id,
                &snapshot_name,
            )],
            json!({ "snapshotName": snapshot_name, "deleted": deleted }),
        ))
    }

    pub async fn diff_schema_snapshot(
        &self,
        input: DiffSchemaSnapshotInput,
    ) -> anyhow::Result<WorkflowEnvelope<WorkflowSchemaDiffOutput>> {
        let snapshot_name = match validate_artifact_name(&input.snapshot_name, "snapshotName") {
            Ok(value) => value,
            Err(error) => {
                return Ok(workflow_failure(
                    "Schema snapshot diff was not produced.",
                    error,
                    None,
                ))
            }
        };
        let (profile, record) = self
            .get_owned_record(
                input.profile,
                input.postgres_version,
                input.database_id,
                input.database_name,
            )
            .await?;
        let snapshot =
            match read_schema_snapshot(&profile.name, &record.database_id, &snapshot_name)? {
                Some(snapshot) => snapshot,
                None => {
                    return Ok(workflow_failure(
                        "Schema snapshot diff was not produced.",
                        workflow_error(
                            "snapshot_not_found",
                            format!("Schema snapshot `{snapshot_name}` does not exist."),
                            Some(
                                "Create it with create_schema_snapshot before diffing.".to_string(),
                            ),
                        ),
                        None,
                    ))
                }
            };
        let connection_string = build_connection_string(
            &profile.admin_url,
            &record.database_name,
            &record.role_name,
            &unprotect_role_password(&record.role_password, &profile)?,
        )?;
        let current = match collect_schema_digest_for_url(&connection_string).await {
            Ok(digest) => digest,
            Err(error) => {
                return Ok(workflow_failure(
                    "Schema snapshot diff was not produced.",
                    schema_snapshot_workflow_error(error),
                    None,
                ))
            }
        };
        if let Some(error) =
            workflow_schema_digest_version_mismatch(&snapshot_name, &snapshot.digest, &current)
        {
            return Ok(workflow_failure(
                "Schema snapshot diff was not produced.",
                error,
                None,
            ));
        }
        let diff = diff_workflow_schema_digests(&snapshot.digest, &current);

        Ok(workflow_success(
            schema_diff_summary(&diff),
            Some(diff.changed_objects.clone()),
            Vec::new(),
            vec![snapshot_detail_handle(
                &profile.name,
                &record.database_id,
                &snapshot_name,
            )],
            diff,
        ))
    }

    pub async fn prepare_for_repo(
        &self,
        input: PrepareForRepoInput,
    ) -> anyhow::Result<WorkflowEnvelope<PrepareForRepoOutput>> {
        let repo_path = PathBuf::from(&input.repo_path);
        if !repo_path.is_dir() {
            return Ok(workflow_failure(
                "Repository was not prepared.",
                workflow_error(
                    "repo_not_found",
                    format!("repoPath is not a directory: {}", repo_path.display()),
                    Some("Pass the absolute path to the repository checkout.".to_string()),
                ),
                None,
            ));
        }

        if let Some(command) = &input.migration_command {
            if let Err(error) = validate_workflow_command(command, "Migration command") {
                return Ok(workflow_failure(
                    "Repository was not prepared.",
                    error,
                    None,
                ));
            }
        }
        if let Some(command) = &input.seed_command {
            if let Err(error) = validate_workflow_command(command, "Seed command") {
                return Ok(workflow_failure(
                    "Repository was not prepared.",
                    error,
                    None,
                ));
            }
        }

        let postgres_version =
            resolve_repo_postgres_version(&repo_path, input.postgres_version.clone())?;
        let sandbox_target = if selector_has_database(&input.database_id, &input.database_name) {
            let connection = self
                .get_connection_string(DatabaseSelector {
                    profile: input.profile.clone(),
                    postgres_version: input.postgres_version.clone(),
                    database_id: input.database_id.clone(),
                    database_name: input.database_name.clone(),
                })
                .await?;
            Some(mask_connection_string(&connection.connection_string))
        } else {
            None
        };

        let existing_project_config = read_repo_project_config(&repo_path)?;
        let migration_command = input.migration_command.or_else(|| {
            existing_project_config
                .as_ref()
                .and_then(|config| config.migration_command.clone())
        });
        let seed_command = input.seed_command.or_else(|| {
            existing_project_config
                .as_ref()
                .and_then(|config| config.seed_command.clone())
        });
        let database_url_env = existing_project_config
            .as_ref()
            .map(|config| config.database_url_env.clone())
            .unwrap_or_else(default_database_url_env);
        let project_config = RepoProjectConfig {
            migration_command,
            seed_command,
            database_url_env,
            postgres_version: postgres_version.version.clone(),
            prepared_at: Utc::now(),
        };
        let migration_command_configured = project_config.migration_command.is_some();
        let seed_command_configured = project_config.seed_command.is_some();
        let config_path = write_repo_project_config(&repo_path, &project_config)?;
        let action_needed = (!migration_command_configured).then(|| {
            "Pass an explicit command to run_repo_command/validate_schema_change or add migrationCommand to .pgsandbox/project.json.".to_string()
        });
        let output = PrepareForRepoOutput {
            repo_path: repo_path.display().to_string(),
            postgres_version: postgres_version.version,
            postgres_version_source: postgres_version.source,
            config_path: Some(config_path.display().to_string()),
            sandbox_target,
            migration_command_configured,
            seed_command_configured,
            action_needed,
        };
        let warnings = output.action_needed.iter().cloned().collect::<Vec<_>>();

        Ok(workflow_success(
            "Repository prepared for PG Sandbox workflows.",
            None,
            warnings,
            vec![json!({
                "type": "repo-config"
            })],
            output,
        ))
    }

    pub async fn run_repo_command(
        &self,
        input: RunRepoCommandInput,
    ) -> anyhow::Result<WorkflowEnvelope<CommandWorkflowOutput>> {
        self.run_repo_schema_command(input).await
    }

    async fn run_repo_schema_command(
        &self,
        input: RunRepoCommandInput,
    ) -> anyhow::Result<WorkflowEnvelope<CommandWorkflowOutput>> {
        if !selector_has_database(&input.database_id, &input.database_name) {
            return Ok(workflow_failure(
                "Repo command was not run.",
                workflow_error(
                    "missing_sandbox",
                    "Repo commands require databaseId or databaseName.",
                    Some("Create a sandbox first, then pass its databaseId.".to_string()),
                ),
                None,
            ));
        }
        let repo_path = PathBuf::from(&input.repo_path);
        if !repo_path.is_dir() {
            return Ok(workflow_failure(
                "Repo command was not run.",
                repo_not_found_error(&repo_path),
                None,
            ));
        }
        let command = match resolve_repo_schema_command(&repo_path, input.command)? {
            Ok(command) => command,
            Err(error) => return Ok(workflow_failure("Repo command was not run.", error, None)),
        };
        let timeout = workflow_timeout(input.timeout_seconds);
        let postgres_version =
            resolve_repo_postgres_version(&repo_path, input.postgres_version.clone())?;
        let connection = self
            .get_connection_string(DatabaseSelector {
                profile: input.profile,
                postgres_version: postgres_version.version,
                database_id: input.database_id,
                database_name: input.database_name,
            })
            .await?;
        let command_result =
            execute_repo_command(&repo_path, &command, &connection.connection_string, timeout)
                .await?;
        let output = command_workflow_output(
            &connection.database_id,
            &connection.database_name,
            command_result,
        );
        let ok = output.exit_code == Some(0);

        Ok(if ok {
            workflow_success(
                "Repo command completed successfully.",
                None,
                Vec::new(),
                vec![json!({
                    "type": "repo-command-run",
                    "databaseId": connection.database_id
                })],
                output,
            )
        } else {
            workflow_failure(
                "Repo command failed.",
                workflow_error(
                    "repo_command_failed",
                    format!("Repo command exited with {:?}", output.exit_code),
                    Some(
                        "Inspect stderr/stdout in the result and rerun after fixing the command."
                            .to_string(),
                    ),
                ),
                Some(output),
            )
        })
    }

    pub async fn validate_schema_change(
        &self,
        input: ValidateSchemaChangeInput,
    ) -> anyhow::Result<WorkflowEnvelope<ValidateSchemaChangeOutput>> {
        self.validate_schema_change_workflow(input).await
    }

    async fn validate_schema_change_workflow(
        &self,
        input: ValidateSchemaChangeInput,
    ) -> anyhow::Result<WorkflowEnvelope<ValidateSchemaChangeOutput>> {
        let repo_path = PathBuf::from(&input.repo_path);
        if !repo_path.is_dir() {
            return Ok(workflow_failure(
                "Schema change validation was not run.",
                repo_not_found_error(&repo_path),
                None,
            ));
        }
        let command = match resolve_repo_schema_command(&repo_path, input.command)? {
            Ok(command) => command,
            Err(error) => {
                return Ok(workflow_failure(
                    "Schema change validation was not run.",
                    error,
                    None,
                ))
            }
        };
        let timeout = workflow_timeout(input.timeout_seconds);
        let postgres_version =
            resolve_repo_postgres_version(&repo_path, input.postgres_version.clone())?;
        let created_sandbox = !selector_has_database(&input.database_id, &input.database_name);
        let mut created_profile = None;
        let connection = if created_sandbox {
            let created = self
                .create_database(CreateDatabaseInput {
                    profile: input.profile,
                    postgres_version: postgres_version.version.clone(),
                    name_hint: Some(
                        input
                            .name_hint
                            .unwrap_or_else(|| "schema change validation".to_string()),
                    ),
                    ttl_minutes: input.ttl_minutes,
                    owner: input.owner,
                    labels: input.labels,
                })
                .await?;
            created_profile = Some(created.profile.clone());
            ConnectionStringOutput {
                database_id: created.database_id,
                database_name: created.database_name,
                expires_at: created.expires_at,
                connection_string_redacted: mask_connection_string(&created.connection_string),
                connection_string: created.connection_string,
            }
        } else {
            self.get_connection_string(DatabaseSelector {
                profile: input.profile,
                postgres_version: postgres_version.version.clone(),
                database_id: input.database_id,
                database_name: input.database_name,
            })
            .await?
        };
        let before = match collect_schema_digest_for_url(&connection.connection_string).await {
            Ok(digest) => digest,
            Err(error) => {
                let summary = if created_sandbox {
                    self.cleanup_auto_created_sandbox(
                        created_profile.clone(),
                        &connection,
                        "schema change validation failed",
                    )
                    .await?;
                    "Schema change validation failed before completion; the created sandbox was deleted."
                } else {
                    "Schema change validation failed before completion."
                };
                return Ok(workflow_failure(
                    summary,
                    schema_snapshot_workflow_error(error),
                    None,
                ));
            }
        };
        let command_result = match execute_repo_command(
            &repo_path,
            &command,
            &connection.connection_string,
            timeout,
        )
        .await
        {
            Ok(result) => result,
            Err(error) if created_sandbox => {
                self.cleanup_auto_created_sandbox(
                    created_profile.clone(),
                    &connection,
                    "schema change validation failed",
                )
                .await?;
                return Ok(workflow_failure(
                        "Schema change validation failed before completion; the created sandbox was deleted.",
                        workflow_error(
                            "schema_change_validation_error",
                            error.to_string(),
                            Some("Retry after fixing the validation error. No sandbox cleanup is required.".to_string()),
                        ),
                        None,
                    ));
            }
            Err(error) => return Err(error),
        };
        let after = match collect_schema_digest_for_url(&connection.connection_string).await {
            Ok(digest) => digest,
            Err(error) => {
                let summary = if created_sandbox {
                    self.cleanup_auto_created_sandbox(
                        created_profile.clone(),
                        &connection,
                        "schema change validation failed",
                    )
                    .await?;
                    "Schema change validation failed before completion; the created sandbox was deleted."
                } else {
                    "Schema change validation failed before completion."
                };
                return Ok(workflow_failure(
                    summary,
                    schema_snapshot_workflow_error(error),
                    None,
                ));
            }
        };
        let diff = diff_workflow_schema_digests(&before, &after);
        let ok = command_result.exit_code == Some(0);
        let output = validate_schema_change_output(
            &connection.database_id,
            &connection.database_name,
            created_sandbox,
            command_result,
            diff.clone(),
        );

        if ok {
            return Ok(workflow_success(
                "Schema change validation completed successfully.",
                Some(diff.changed_objects),
                Vec::new(),
                vec![json!({
                    "type": "schema-change-validation",
                    "databaseId": connection.database_id,
                    "createdSandbox": created_sandbox
                })],
                output,
            ));
        }

        let deleted_auto_sandbox = if created_sandbox {
            self.cleanup_auto_created_sandbox(
                created_profile,
                &connection,
                "schema change validation failed",
            )
            .await?;
            true
        } else {
            false
        };

        Ok(workflow_failure_with_changes(
            if deleted_auto_sandbox {
                "Schema change validation failed; the created sandbox was deleted."
            } else {
                "Schema change validation failed."
            },
            diff.changed_objects,
            workflow_error(
                "repo_command_failed",
                format!("Repo command exited with {:?}", output.exit_code),
                Some(
                    if deleted_auto_sandbox {
                        "Inspect stderr/stdout and rerun after fixing the command. No sandbox cleanup is required."
                    } else {
                        "Inspect stderr/stdout in the result and the schema diff before retrying."
                    }
                    .to_string(),
                ),
            ),
            Some(output),
        ))
    }

    async fn cleanup_auto_created_sandbox(
        &self,
        created_profile: Option<String>,
        connection: &ConnectionStringOutput,
        context: &str,
    ) -> anyhow::Result<()> {
        self.delete_database(DatabaseSelector {
            profile: created_profile,
            postgres_version: None,
            database_id: Some(connection.database_id.clone()),
            database_name: None,
        })
        .await
        .map(|_| ())
        .map_err(|cleanup_error| {
            anyhow::anyhow!(
                "{context} and cleanup also failed for {}: cleanup error: {cleanup_error}",
                connection.database_name
            )
        })
    }

    pub async fn seed_database(
        &self,
        input: SeedDatabaseInput,
    ) -> anyhow::Result<WorkflowEnvelope<CommandWorkflowOutput>> {
        if !selector_has_database(&input.database_id, &input.database_name) {
            return Ok(workflow_failure(
                "Seed command was not run.",
                workflow_error(
                    "missing_sandbox",
                    "seed_database requires databaseId or databaseName.",
                    Some("Create a sandbox first, then pass its databaseId.".to_string()),
                ),
                None,
            ));
        }
        let repo_path = PathBuf::from(&input.repo_path);
        if !repo_path.is_dir() {
            return Ok(workflow_failure(
                "Seed command was not run.",
                repo_not_found_error(&repo_path),
                None,
            ));
        }
        let command = match resolve_seed_command(&repo_path, input.command)? {
            Ok(command) => command,
            Err(error) => return Ok(workflow_failure("Seed command was not run.", error, None)),
        };
        let timeout = workflow_timeout(input.timeout_seconds);
        let postgres_version =
            resolve_repo_postgres_version(&repo_path, input.postgres_version.clone())?;
        let connection = self
            .get_connection_string(DatabaseSelector {
                profile: input.profile,
                postgres_version: postgres_version.version,
                database_id: input.database_id,
                database_name: input.database_name,
            })
            .await?;
        let command_result =
            execute_repo_command(&repo_path, &command, &connection.connection_string, timeout)
                .await?;
        let output = command_workflow_output(
            &connection.database_id,
            &connection.database_name,
            command_result,
        );

        Ok(if output.exit_code == Some(0) {
            workflow_success(
                "Seed command completed successfully.",
                None,
                Vec::new(),
                vec![json!({
                    "type": "seed-run",
                    "databaseId": connection.database_id
                })],
                output,
            )
        } else {
            workflow_failure(
                "Seed command failed.",
                workflow_error(
                    "seed_failed",
                    format!("Seed command exited with {:?}", output.exit_code),
                    Some("Inspect stderr/stdout in the result before retrying.".to_string()),
                ),
                Some(output),
            )
        })
    }

    pub async fn create_template_from_sandbox(
        &self,
        input: CreateTemplateFromSandboxInput,
    ) -> anyhow::Result<WorkflowEnvelope<CreateTemplateOutput>> {
        let template_name = match validate_artifact_name(&input.template_name, "templateName") {
            Ok(value) => value,
            Err(error) => return Ok(workflow_failure("Template was not created.", error, None)),
        };
        let (profile, record) = self
            .get_owned_record(
                input.profile,
                input.postgres_version,
                input.database_id,
                input.database_name,
            )
            .await?;
        let connection_string = build_connection_string(
            &profile.admin_url,
            &record.database_name,
            &record.role_name,
            &unprotect_role_password(&record.role_password, &profile)?,
        )?;
        let (client, connection_task) = connect_url(&connection_string).await?;
        let postgres_version = postgres_version(&client).await?;
        drop(client);
        let _ = connection_task.await;

        let paths = template_paths(&profile.name, &template_name)?;
        dump_database_to_file(&connection_string, &paths.dump_path).await?;
        let size_bytes = fs::metadata(&paths.dump_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let metadata = TemplateMetadata {
            template_name: template_name.clone(),
            profile: profile.name.clone(),
            source_sandbox_id: record.database_id.clone(),
            source_database_name: record.database_name.clone(),
            created_at: Utc::now(),
            created_by: input.created_by,
            owner: record.owner.clone(),
            postgres_version,
            size_bytes,
            notes: input.notes,
            privacy_warning: TEMPLATE_PRIVACY_WARNING.to_string(),
        };
        write_json_file(&paths.metadata_path, &metadata)?;

        Ok(workflow_success(
            format!("Template `{template_name}` created."),
            None,
            vec![TEMPLATE_PRIVACY_WARNING.to_string()],
            vec![template_detail_handle(&profile.name, &template_name)],
            CreateTemplateOutput { metadata },
        ))
    }

    pub async fn create_sandbox_from_template(
        &self,
        input: CreateSandboxFromTemplateInput,
    ) -> anyhow::Result<WorkflowEnvelope<CreateSandboxFromTemplateOutput>> {
        let template_name = match validate_artifact_name(&input.template_name, "templateName") {
            Ok(value) => value,
            Err(error) => {
                return Ok(workflow_failure(
                    "Sandbox was not created from template.",
                    error,
                    None,
                ))
            }
        };
        let profile =
            self.resolve_profile(input.profile.as_deref(), input.postgres_version.as_deref())?;
        let paths = template_paths(&profile.name, &template_name)?;
        let metadata = match read_json_file::<TemplateMetadata>(&paths.metadata_path)? {
            Some(metadata) => metadata,
            None => {
                return Ok(workflow_failure(
                    "Sandbox was not created from template.",
                    workflow_error(
                        "template_not_found",
                        format!("Template `{template_name}` does not exist for profile {}.", profile.name),
                        Some("Create it with create_template_from_sandbox or choose another templateName.".to_string()),
                    ),
                    None,
                ))
            }
        };
        let created = self
            .create_database(CreateDatabaseInput {
                profile: Some(profile.name.clone()),
                postgres_version: None,
                name_hint: input.name_hint.or_else(|| Some(template_name.clone())),
                ttl_minutes: input.ttl_minutes,
                owner: input.owner,
                labels: input.labels,
            })
            .await?;
        if let Err(error) =
            restore_database_from_file(&paths.dump_path, &created.connection_string).await
        {
            let cleanup_result = self
                .delete_database(DatabaseSelector {
                    profile: Some(created.profile.clone()),
                    postgres_version: None,
                    database_id: Some(created.database_id.clone()),
                    database_name: None,
                })
                .await;
            return match cleanup_result {
                Ok(_) => Ok(workflow_failure(
                    "Sandbox restore failed; created sandbox was deleted.",
                    workflow_error(
                        "template_restore_failed",
                        error.to_string(),
                        Some("Check that pg_restore is installed and the template artifact is valid.".to_string()),
                    ),
                    None,
                )),
                Err(cleanup_error) => Err(anyhow::anyhow!(
                    "template restore failed and cleanup also failed for {}: {error}; cleanup error: {cleanup_error}",
                    created.database_name
                )),
            };
        }

        let created_sandbox = CreateSandboxFromTemplateOutput {
            database_id: created.database_id,
            profile: created.profile,
            database_name: created.database_name,
            role_name: created.role_name,
            expires_at: created.expires_at,
            connection_string_redacted: mask_connection_string(&created.connection_string),
            connection_string: created.connection_string,
            template_name: template_name.clone(),
        };
        let mut envelope = workflow_success(
            format!(
                "Sandbox created from template `{}`.",
                metadata.template_name
            ),
            None,
            vec![metadata.privacy_warning.clone()],
            vec![template_detail_handle(&profile.name, &template_name)],
            created_sandbox.clone(),
        );
        envelope.created_sandbox = Some(created_sandbox);
        Ok(envelope)
    }

    pub async fn list_templates(
        &self,
        input: ListTemplatesInput,
    ) -> anyhow::Result<WorkflowEnvelope<Vec<TemplateMetadata>>> {
        let profile =
            self.resolve_profile(input.profile.as_deref(), input.postgres_version.as_deref())?;
        let templates = read_templates(&profile.name)?;

        Ok(workflow_success(
            format!("Found {} template(s).", templates.len()),
            None,
            Vec::new(),
            vec![json!({
                "type": "template-list",
                "profile": profile.name
            })],
            templates,
        ))
    }

    pub async fn delete_template(
        &self,
        input: DeleteTemplateInput,
    ) -> anyhow::Result<WorkflowEnvelope<DeleteTemplateOutput>> {
        let template_name = match validate_artifact_name(&input.template_name, "templateName") {
            Ok(value) => value,
            Err(error) => return Ok(workflow_failure("Template was not deleted.", error, None)),
        };
        let profile =
            self.resolve_profile(input.profile.as_deref(), input.postgres_version.as_deref())?;
        let paths = template_paths(&profile.name, &template_name)?;
        let deleted_dump = remove_file_if_exists(&paths.dump_path)?;
        let deleted_metadata = remove_file_if_exists(&paths.metadata_path)?;
        let deleted = deleted_dump || deleted_metadata;

        Ok(workflow_success(
            if deleted {
                format!("Template `{template_name}` deleted.")
            } else {
                format!("Template `{template_name}` did not exist.")
            },
            None,
            Vec::new(),
            vec![template_detail_handle(&profile.name, &template_name)],
            DeleteTemplateOutput {
                template_name,
                deleted,
            },
        ))
    }

    pub async fn cleanup_expired(
        &self,
        input: CleanupExpiredInput,
    ) -> anyhow::Result<CleanupExpiredOutput> {
        let dry_run = input.dry_run.unwrap_or(false);
        if all_versions_requested(
            input.postgres_version.as_deref(),
            input.include_all_versions,
        ) {
            if input.profile.is_some() {
                anyhow::bail!(
                    "includeAllVersions/postgresVersion=\"*\" cannot be combined with profile; omit profile to clean up configured profiles and running managed-local versions."
                );
            }
            let profiles = self.profiles_for_all_version_operations()?;
            let profile_names = profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect::<Vec<_>>();
            let mut selected = Vec::new();
            let mut deleted = Vec::new();
            let mut failures = Vec::new();
            for profile in &profiles {
                match self.cleanup_expired_for_profile(profile, dry_run).await {
                    Ok(result) => {
                        if let Some(profile_selected) = result.selected {
                            selected.extend(profile_selected);
                        }
                        if let Some(profile_deleted) = result.deleted {
                            deleted.extend(profile_deleted);
                        }
                        if let Some(profile_failures) = result.failures {
                            failures.extend(profile_failures);
                        }
                    }
                    Err(error) => failures.push(profile_operation_failure(profile, error)),
                }
            }
            return Ok(CleanupExpiredOutput {
                scope: "allVersions".to_string(),
                profile: None,
                profiles: profile_names,
                remaining_profiles: Vec::new(),
                dry_run,
                selected: dry_run.then_some(selected),
                deleted: (!dry_run).then_some(deleted),
                failures: (!failures.is_empty() || !dry_run).then_some(failures),
            });
        }

        let profile =
            self.resolve_profile(input.profile.as_deref(), input.postgres_version.as_deref())?;
        let mut result = self.cleanup_expired_for_profile(&profile, dry_run).await?;
        result.remaining_profiles = self
            .profile_names_for_scope_hint()
            .into_iter()
            .filter(|name| name != &profile.name)
            .collect();
        Ok(result)
    }

    async fn cleanup_expired_for_profile(
        &self,
        profile: &SandboxProfile,
        dry_run: bool,
    ) -> anyhow::Result<CleanupExpiredOutput> {
        let (client, connection_task) = connect_admin(profile).await?;
        ensure_metadata_table(&client).await?;
        let expired = client
            .query(
                &format!(
                    r#"
                      SELECT *
                      FROM {}
                      WHERE profile_name = $1
                        AND deleted_at IS NULL
                        AND expires_at <= now()
                      ORDER BY expires_at ASC
                      LIMIT 50
                    "#,
                    quote_ident(METADATA_TABLE)?
                ),
                &[&profile.name],
            )
            .await?;
        let records = expired
            .iter()
            .map(sandbox_record_from_row)
            .collect::<Vec<_>>();

        if dry_run {
            drop(client);
            let _ = connection_task.await;
            return Ok(CleanupExpiredOutput {
                scope: "profile".to_string(),
                profile: Some(profile.name.clone()),
                profiles: vec![profile.name.clone()],
                remaining_profiles: Vec::new(),
                dry_run: true,
                selected: Some(records.iter().map(record_to_json).collect()),
                deleted: None,
                failures: None,
            });
        }

        let mut deleted = Vec::new();
        let mut failures = Vec::new();
        for record in records {
            let deletion = async {
                terminate_database_connections(&client, &record.database_name).await?;
                client
                    .batch_execute(&format!(
                        "DROP DATABASE IF EXISTS {}",
                        quote_ident(&record.database_name)?
                    ))
                    .await?;
                client
                    .batch_execute(&format!(
                        "DROP ROLE IF EXISTS {}",
                        quote_ident(&record.role_name)?
                    ))
                    .await?;
                client
                    .execute(
                        &format!(
                            "UPDATE {} SET deleted_at = now() WHERE database_id = $1",
                            quote_ident(METADATA_TABLE)?
                        ),
                        &[&record.database_id],
                    )
                    .await?;
                anyhow::Ok(())
            }
            .await;

            match deletion {
                Ok(()) => {
                    deleted.push(record.database_id.clone());
                    let _ = record_audit_event(
                        &client,
                        "cleanup_expired",
                        &profile.name,
                        &record.database_id,
                        &record.database_name,
                        Some(&record.role_name),
                        json!({ "expiresAt": record.expires_at }),
                    )
                    .await;
                }
                Err(error) => {
                    let message = error.to_string();
                    let _ = record_audit_event(
                        &client,
                        "cleanup_expired_failed",
                        &profile.name,
                        &record.database_id,
                        &record.database_name,
                        Some(&record.role_name),
                        json!({ "message": message.clone() }),
                    )
                    .await;
                    failures.push(json!({
                        "databaseId": record.database_id,
                        "message": message
                    }));
                }
            }
        }

        drop(client);
        let _ = connection_task.await;

        Ok(CleanupExpiredOutput {
            scope: "profile".to_string(),
            profile: Some(profile.name.clone()),
            profiles: vec![profile.name.clone()],
            remaining_profiles: Vec::new(),
            dry_run: false,
            selected: None,
            deleted: Some(deleted),
            failures: Some(failures),
        })
    }

    fn profiles_for_all_version_operations(&self) -> anyhow::Result<Vec<SandboxProfile>> {
        let mut profiles = Vec::new();
        for profile in &self.config.profiles {
            let resolved = if profile.managed_local {
                self.running_managed_local_profile(profile.postgres_version.as_deref())
            } else {
                Some(profile.clone())
            };
            if let Some(resolved) = resolved {
                push_unique_profile(&mut profiles, resolved);
            }
        }
        if self.config.managed_local.enabled {
            for installation in discover_local_postgres_installations() {
                if let Some(profile) =
                    self.running_managed_local_profile(Some(&installation.postgres_version))
                {
                    push_unique_profile(&mut profiles, profile);
                }
            }
        }
        Ok(profiles)
    }

    fn running_managed_local_profile(
        &self,
        postgres_version: Option<&str>,
    ) -> Option<SandboxProfile> {
        let cluster = LocalPostgresCluster::from_env_for_version(postgres_version).ok()?;
        let status = cluster.status().ok()?;
        if !status.running {
            return None;
        }
        status
            .config
            .map(|local_config| self.profile_from_local_config(local_config))
    }

    fn profile_names_for_scope_hint(&self) -> Vec<String> {
        let mut names = self
            .config
            .profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect::<Vec<_>>();
        if self.config.managed_local.enabled {
            for installation in discover_local_postgres_installations() {
                let name = format!("local-pg{}", installation.postgres_version);
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }
}

fn unknown_postgres_version_error(config: &SandboxConfig, version: &str) -> anyhow::Error {
    let default_profile = config
        .profiles
        .iter()
        .find(|profile| profile.name == config.default_profile);
    let profile_summary = default_profile
        .map(|profile| {
            format!(
                "{} (managedLocal={}, postgresVersion={})",
                profile.name,
                profile.managed_local,
                profile.postgres_version.as_deref().unwrap_or("unspecified")
            )
        })
        .unwrap_or_else(|| config.default_profile.clone());

    anyhow::anyhow!(
        "No configured profile advertises postgresVersion {version}. The active default profile is {profile_summary}. To use managed local version discovery, rerun `pgsandbox-mcp setup --client <client>` without --admin-url, restart the MCP client, and retry. Or add an explicit profile with postgresVersion {version}."
    )
}

fn list_profile_hints(config: &SandboxConfig) -> Vec<String> {
    let mut hints = vec![restart_required_after_setup_note()];
    if !config.managed_local.enabled {
        hints.push(
            "This server is using explicit configured Postgres profile(s), not managed local version discovery. If this was accidental or stale, rerun `pgsandbox-mcp setup --client <client>` without --admin-url and restart the MCP client.".to_string(),
        );
    }
    hints
}

fn restart_required_after_setup_note() -> String {
    format!(
        "MCP clients cache tool metadata. After setup or upgrade, restart the MCP client and verify pgsandbox-mcp reports {} tools.",
        PUBLIC_MCP_TOOL_COUNT
    )
}

fn profile_operation_failure(profile: &SandboxProfile, error: anyhow::Error) -> Value {
    json!({
        "profile": profile.name,
        "category": "profile_unavailable",
        "message": format!("{error:#}"),
    })
}

fn profile_admin_url_summary(profile: &SandboxProfile) -> String {
    if profile.managed_local && profile.admin_url == DEFERRED_LOCAL_ADMIN_URL {
        "(managed local; starts on demand)".to_string()
    } else {
        mask_connection_string(&profile.admin_url)
    }
}

fn profile_admin_url_port(profile: &SandboxProfile) -> Option<u16> {
    if profile.managed_local && profile.admin_url == DEFERRED_LOCAL_ADMIN_URL {
        return None;
    }
    Url::parse(&profile.admin_url)
        .ok()
        .and_then(|url| url.port())
}

fn workflow_success<T: Serialize>(
    summary: impl Into<String>,
    changed_objects: Option<SchemaChangeCounts>,
    warnings: Vec<String>,
    detail_handles: Vec<Value>,
    result: T,
) -> WorkflowEnvelope<T> {
    WorkflowEnvelope {
        ok: true,
        summary: summary.into(),
        changed_objects,
        warnings,
        errors: Vec::new(),
        detail_handles,
        result: Some(result),
        created_sandbox: None,
    }
}

fn workflow_failure<T: Serialize>(
    summary: impl Into<String>,
    error: WorkflowError,
    result: Option<T>,
) -> WorkflowEnvelope<T> {
    workflow_failure_with_changes(summary, SchemaChangeCounts::default(), error, result)
}

fn workflow_failure_with_changes<T: Serialize>(
    summary: impl Into<String>,
    changed_objects: SchemaChangeCounts,
    error: WorkflowError,
    result: Option<T>,
) -> WorkflowEnvelope<T> {
    WorkflowEnvelope {
        ok: false,
        summary: summary.into(),
        changed_objects: Some(changed_objects),
        warnings: Vec::new(),
        errors: vec![error],
        detail_handles: Vec::new(),
        result,
        created_sandbox: None,
    }
}

fn workflow_error(
    code: impl Into<String>,
    message: impl Into<String>,
    hint: Option<String>,
) -> WorkflowError {
    let code = code.into();
    WorkflowError {
        category: workflow_error_category(&code).to_string(),
        code,
        message: message.into(),
        hint,
    }
}

fn workflow_error_category(code: &str) -> &'static str {
    match code {
        "template_not_found" => "template_not_found",
        "template_restore_failed" => "restore_failed",
        "missing_seed_command"
        | "missing_schema_change_command"
        | "unsafe_command"
        | "unclear_command"
        | "invalid_artifact_name"
        | "repo_not_found" => "validation",
        "repo_command_failed" | "seed_failed" => "command_failed",
        "schema_change_validation_error" | "schema_snapshot_failed" | "schema_snapshot_timeout" => {
            "workflow"
        }
        _ => "workflow",
    }
}

fn repo_not_found_error(repo_path: &Path) -> WorkflowError {
    workflow_error(
        "repo_not_found",
        format!("repoPath is not a directory: {}", repo_path.display()),
        Some("Pass the absolute path to the repository checkout.".to_string()),
    )
}

fn selector_has_database(database_id: &Option<String>, database_name: &Option<String>) -> bool {
    database_id.is_some() || database_name.is_some()
}

fn selector_is_unscoped_database_id(
    profile_name: Option<&String>,
    postgres_version: Option<&String>,
    database_id: Option<&String>,
    database_name: Option<&String>,
) -> bool {
    profile_name.is_none()
        && postgres_version.is_none()
        && database_id.is_some()
        && database_name.is_none()
}

fn selector_is_unscoped_database_name(
    profile_name: Option<&String>,
    postgres_version: Option<&String>,
    database_id: Option<&String>,
    database_name: Option<&String>,
) -> bool {
    profile_name.is_none()
        && postgres_version.is_none()
        && database_id.is_none()
        && database_name.is_some()
}

fn all_versions_requested(
    postgres_version: Option<&str>,
    include_all_versions: Option<bool>,
) -> bool {
    include_all_versions.unwrap_or(false)
        || postgres_version.is_some_and(|value| value.trim() == "*")
}

fn push_unique_profile(profiles: &mut Vec<SandboxProfile>, profile: SandboxProfile) {
    if profiles
        .iter()
        .all(|existing| existing.name != profile.name)
    {
        profiles.push(profile);
    }
}

async fn collect_schema_digest_for_url(database_url: &str) -> anyhow::Result<WorkflowSchemaDigest> {
    let (client, connection_task) = connect_url(database_url).await?;
    let digest = schema_phase_timeout("schema digest", collect_schema_digest(&client)).await;
    drop(client);
    finish_connection_task(connection_task).await;
    digest
}

async fn collect_schema_snapshot_digest_for_url(
    database_url: &str,
) -> anyhow::Result<(String, WorkflowSchemaDigest)> {
    let (client, connection_task) = connect_url(database_url).await?;
    let result = async {
        let postgres_version =
            schema_phase_timeout("postgres version", postgres_version(&client)).await?;
        let digest = schema_phase_timeout("schema digest", collect_schema_digest(&client)).await?;
        anyhow::Ok((postgres_version, digest))
    }
    .await;
    drop(client);
    finish_connection_task(connection_task).await;
    result
}

async fn schema_phase_timeout<T, Fut>(phase: &'static str, operation: Fut) -> anyhow::Result<T>
where
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let timeout = StdDuration::from_secs(DEFAULT_SCHEMA_OPERATION_TIMEOUT_SECONDS);
    match time::timeout(timeout, operation).await {
        Ok(result) => result,
        Err(_) => anyhow::bail!(
            "schema_operation_timeout: {phase} exceeded {} seconds",
            timeout.as_secs()
        ),
    }
}

async fn finish_connection_task(
    mut connection_task: tokio::task::JoinHandle<Result<(), tokio_postgres::Error>>,
) {
    let timeout = StdDuration::from_secs(CONNECTION_TASK_CLOSE_TIMEOUT_SECONDS);
    tokio::select! {
        _ = &mut connection_task => {}
        _ = time::sleep(timeout) => {
            connection_task.abort();
            let _ = connection_task.await;
        }
    }
}

fn schema_snapshot_workflow_error(error: anyhow::Error) -> WorkflowError {
    let message = error.to_string();
    if message.contains("schema_operation_timeout") {
        workflow_error(
            "schema_snapshot_timeout",
            message,
            Some(
                "Retry after checking for blocked schema locks; PGSandbox bounded this snapshot phase instead of waiting for the MCP client timeout."
                    .to_string(),
            ),
        )
    } else {
        workflow_error(
            "schema_snapshot_failed",
            message,
            Some(
                "Run describe_schema or schema_digest to verify the sandbox schema, then retry snapshot creation."
                    .to_string(),
            ),
        )
    }
}

async fn collect_schema_digest(client: &Client) -> anyhow::Result<WorkflowSchemaDigest> {
    let table_rows = client
        .query(
            r#"
              SELECT n.nspname AS table_schema,
                     c.relname AS table_name,
                     CASE c.relkind
                       WHEN 'r' THEN 'table'
                       WHEN 'p' THEN 'partitioned_table'
                       WHEN 'v' THEN 'view'
                       WHEN 'm' THEN 'materialized_view'
                       WHEN 'f' THEN 'foreign_table'
                       ELSE c.relkind::text
                     END AS relation_kind,
                     CASE WHEN c.relkind IN ('v', 'm') THEN pg_get_viewdef(c.oid, true) ELSE NULL END AS view_definition
              FROM pg_class c
              JOIN pg_namespace n ON n.oid = c.relnamespace
              WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f')
                AND n.nspname NOT IN ('pg_catalog', 'information_schema')
              ORDER BY n.nspname, c.relname
            "#,
            &[],
        )
        .await?;
    let column_rows = client
        .query(
            r#"
              SELECT n.nspname AS table_schema,
                     c.relname AS table_name,
                     a.attname AS column_name,
                     a.attnum::integer AS ordinal_position,
                     pg_catalog.format_type(a.atttypid, a.atttypmod) AS data_type,
                     t.typname AS udt_name,
                     CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END AS is_nullable,
                     CASE WHEN a.attgenerated = '' THEN pg_get_expr(ad.adbin, ad.adrelid) ELSE NULL END AS column_default,
                     CASE WHEN a.attgenerated = '' THEN NULL ELSE a.attgenerated::text END AS generated_kind,
                     CASE WHEN a.attgenerated = '' THEN NULL ELSE pg_get_expr(ad.adbin, ad.adrelid) END AS generation_expression
              FROM pg_attribute a
              JOIN pg_class c ON c.oid = a.attrelid
              JOIN pg_namespace n ON n.oid = c.relnamespace
              JOIN pg_type t ON t.oid = a.atttypid
              LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum
              WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f')
                AND a.attnum > 0
                AND NOT a.attisdropped
                AND n.nspname NOT IN ('pg_catalog', 'information_schema')
              ORDER BY n.nspname, c.relname, a.attnum
            "#,
            &[],
        )
        .await?;
    let constraint_rows = client
        .query(
            r#"
              SELECT n.nspname AS table_schema,
                     c.relname AS table_name,
                     con.conname AS constraint_name,
                     CASE con.contype
                       WHEN 'p' THEN 'primary_key'
                       WHEN 'u' THEN 'unique'
                       WHEN 'f' THEN 'foreign_key'
                       WHEN 'c' THEN 'check'
                       WHEN 'x' THEN 'exclusion'
                       ELSE con.contype::text
                     END AS constraint_type,
                     pg_get_constraintdef(con.oid, true) AS definition,
                     CASE con.confupdtype
                       WHEN 'a' THEN 'no_action'
                       WHEN 'r' THEN 'restrict'
                       WHEN 'c' THEN 'cascade'
                       WHEN 'n' THEN 'set_null'
                       WHEN 'd' THEN 'set_default'
                       ELSE NULL
                     END AS update_action,
                     CASE con.confdeltype
                       WHEN 'a' THEN 'no_action'
                       WHEN 'r' THEN 'restrict'
                       WHEN 'c' THEN 'cascade'
                       WHEN 'n' THEN 'set_null'
                       WHEN 'd' THEN 'set_default'
                       ELSE NULL
                     END AS delete_action
              FROM pg_constraint con
              JOIN pg_class c ON c.oid = con.conrelid
              JOIN pg_namespace n ON n.oid = c.relnamespace
              WHERE c.relkind IN ('r', 'p')
                AND n.nspname NOT IN ('pg_catalog', 'information_schema')
              ORDER BY n.nspname, c.relname, con.conname
            "#,
            &[],
        )
        .await?;
    let index_rows = client
        .query(
            r#"
              SELECT schemaname, tablename, indexname, indexdef
              FROM pg_indexes
              WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
              ORDER BY schemaname, tablename, indexname
            "#,
            &[],
        )
        .await?;
    let extension_rows = client
        .query(
            "SELECT extname, extversion FROM pg_extension ORDER BY extname",
            &[],
        )
        .await?;

    let mut tables = Vec::new();
    for row in table_rows {
        let schema: String = row.get("table_schema");
        let name: String = row.get("table_name");
        let relation_kind: String = row.get("relation_kind");
        let view_definition: Option<String> = row.get("view_definition");
        tables.push(schema_object_digest(
            relation_kind.clone(),
            format!("{schema}.{name}"),
            json!({
                "schema": schema,
                "name": name,
                "relationKind": relation_kind,
                "viewDefinitionHash": view_definition
                    .as_deref()
                    .map(|definition| sha256_hex(definition.as_bytes()))
            }),
        )?);
    }

    let mut columns = Vec::new();
    for row in column_rows {
        let schema: String = row.get("table_schema");
        let table: String = row.get("table_name");
        let name: String = row.get("column_name");
        let ordinal_position: i32 = row
            .try_get("ordinal_position")
            .context("schema digest column ordinal position was not an integer")?;
        let data_type: String = row.get("data_type");
        let udt_name: String = row.get("udt_name");
        let is_nullable: String = row.get("is_nullable");
        let column_default: Option<String> = row.get("column_default");
        let generated_kind: Option<String> = row.get("generated_kind");
        let generation_expression: Option<String> = row.get("generation_expression");
        columns.push(schema_object_digest(
            "column",
            format!("{schema}.{table}.{name}"),
            json!({
                "schema": schema,
                "table": table,
                "name": name,
                "ordinalPosition": ordinal_position,
                "dataType": data_type,
                "udtName": udt_name,
                "isNullable": is_nullable,
                "columnDefault": column_default,
                "generatedKind": generated_kind,
                "generationExpression": generation_expression
            }),
        )?);
    }

    let mut constraints = Vec::new();
    for row in constraint_rows {
        let schema: String = row.get("table_schema");
        let table: String = row.get("table_name");
        let name: String = row.get("constraint_name");
        let constraint_type: String = row.get("constraint_type");
        let definition: String = row.get("definition");
        let update_action: Option<String> = row.get("update_action");
        let delete_action: Option<String> = row.get("delete_action");
        constraints.push(schema_object_digest(
            "constraint",
            format!("{schema}.{table}.{name}"),
            json!({
                "schema": schema,
                "table": table,
                "name": name,
                "constraintType": constraint_type,
                "definitionHash": sha256_hex(definition.as_bytes()),
                "updateAction": update_action,
                "deleteAction": delete_action
            }),
        )?);
    }

    let mut indexes = Vec::new();
    for row in index_rows {
        let schema: String = row.get("schemaname");
        let table: String = row.get("tablename");
        let name: String = row.get("indexname");
        let definition: String = row.get("indexdef");
        indexes.push(schema_object_digest(
            "index",
            format!("{schema}.{table}.{name}"),
            json!({
                "schema": schema,
                "table": table,
                "name": name,
                "definition": definition
            }),
        )?);
    }

    let mut extensions = Vec::new();
    for row in extension_rows {
        let name: String = row.get("extname");
        let version: String = row.get("extversion");
        extensions.push(schema_object_digest(
            "extension",
            name.clone(),
            json!({
                "name": name,
                "version": version
            }),
        )?);
    }

    let relation_counts = relation_counts_for_schema_objects(&tables);
    let object_counts = SchemaObjectCounts {
        tables: relation_counts.tables,
        partitioned_tables: relation_counts.partitioned_tables,
        views: relation_counts.views,
        materialized_views: relation_counts.materialized_views,
        foreign_tables: relation_counts.foreign_tables,
        columns: columns.len(),
        constraints: constraints.len(),
        indexes: indexes.len(),
        extensions: extensions.len(),
    };
    let canonical = json!({
        "digestVersion": SCHEMA_DIGEST_VERSION,
        "objectCounts": object_counts.clone(),
        "tables": tables.clone(),
        "columns": columns.clone(),
        "constraints": constraints.clone(),
        "indexes": indexes.clone(),
        "extensions": extensions.clone()
    });
    let fingerprint = fingerprint_json(&canonical)?;

    Ok(WorkflowSchemaDigest {
        digest_version: SCHEMA_DIGEST_VERSION,
        fingerprint,
        object_counts,
        tables,
        columns,
        constraints,
        indexes,
        extensions,
    })
}

fn schema_object_digest(
    kind: impl Into<String>,
    key: impl Into<String>,
    summary: Value,
) -> anyhow::Result<SchemaObjectDigest> {
    let kind = kind.into();
    let key = key.into();
    let fingerprint = fingerprint_json(&json!({
        "kind": kind,
        "key": key,
        "summary": summary
    }))?;
    Ok(SchemaObjectDigest {
        kind,
        key,
        fingerprint,
        summary,
    })
}

fn fingerprint_json(value: &Value) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(value)?;
    let digest = Sha256::digest(bytes);
    Ok(bytes_to_hex(&digest))
}

fn diff_workflow_schema_digests(
    from: &WorkflowSchemaDigest,
    to: &WorkflowSchemaDigest,
) -> WorkflowSchemaDiffOutput {
    let from_objects = schema_object_map(from);
    let to_objects = schema_object_map(to);
    let mut added_all = Vec::new();
    let mut removed_all = Vec::new();
    let mut changed_all = Vec::new();

    for (key, object) in &to_objects {
        if !from_objects.contains_key(key) {
            added_all.push(diff_item(object));
        }
    }
    for (key, object) in &from_objects {
        match to_objects.get(key) {
            Some(after) if after.fingerprint != object.fingerprint => {
                changed_all.push(WorkflowSchemaDiffChange {
                    kind: object.kind.clone(),
                    key: object.key.clone(),
                    before_fingerprint: object.fingerprint.clone(),
                    after_fingerprint: after.fingerprint.clone(),
                });
            }
            None => removed_all.push(diff_item(object)),
            _ => {}
        }
    }

    let changed_objects = SchemaChangeCounts {
        added: added_all.len(),
        removed: removed_all.len(),
        changed: changed_all.len(),
    };
    let truncated = added_all.len() > MAX_SCHEMA_DIFF_ITEMS
        || removed_all.len() > MAX_SCHEMA_DIFF_ITEMS
        || changed_all.len() > MAX_SCHEMA_DIFF_ITEMS;

    WorkflowSchemaDiffOutput {
        from_fingerprint: from.fingerprint.clone(),
        to_fingerprint: to.fingerprint.clone(),
        changed_objects,
        added: added_all.into_iter().take(MAX_SCHEMA_DIFF_ITEMS).collect(),
        removed: removed_all
            .into_iter()
            .take(MAX_SCHEMA_DIFF_ITEMS)
            .collect(),
        changed: changed_all
            .into_iter()
            .take(MAX_SCHEMA_DIFF_ITEMS)
            .collect(),
        truncated,
    }
}

fn workflow_schema_digest_version_mismatch(
    snapshot_name: &str,
    snapshot_digest: &WorkflowSchemaDigest,
    current_digest: &WorkflowSchemaDigest,
) -> Option<WorkflowError> {
    if snapshot_digest.digest_version == current_digest.digest_version {
        return None;
    }

    Some(workflow_error(
        "schema_digest_version_mismatch",
        format!(
            "Schema snapshot `{snapshot_name}` was created with schema digest v{} but the current schema digest uses v{}.",
            snapshot_digest.digest_version, current_digest.digest_version
        ),
        Some(
            "Delete this snapshot and create a new baseline with create_schema_snapshot before diffing."
                .to_string(),
        ),
    ))
}

fn schema_object_map(digest: &WorkflowSchemaDigest) -> BTreeMap<String, SchemaObjectDigest> {
    digest
        .tables
        .iter()
        .chain(digest.columns.iter())
        .chain(digest.constraints.iter())
        .chain(digest.indexes.iter())
        .chain(digest.extensions.iter())
        .map(|object| (format!("{}\0{}", object.kind, object.key), object.clone()))
        .collect()
}

fn diff_item(object: &SchemaObjectDigest) -> WorkflowSchemaDiffItem {
    WorkflowSchemaDiffItem {
        kind: object.kind.clone(),
        key: object.key.clone(),
    }
}

fn schema_diff_summary(diff: &WorkflowSchemaDiffOutput) -> String {
    format!(
        "Schema diff: {} added, {} removed, {} changed.",
        diff.changed_objects.added, diff.changed_objects.removed, diff.changed_objects.changed
    )
}

async fn postgres_version(client: &Client) -> anyhow::Result<String> {
    let row = client.query_one("SHOW server_version", &[]).await?;
    Ok(row.get::<_, String>(0))
}

async fn preflight_clone_compatibility(
    source_database_url: &str,
    target_profile: &SandboxProfile,
) -> anyhow::Result<()> {
    let source_version = postgres_server_version_for_url(source_database_url)
        .await
        .context("failed to inspect source Postgres version before clone")?;
    let source_major = postgres_major_from_server_version(&source_version)?;
    let target_major = postgres_major_for_profile(target_profile).await?;

    if clone_downgrade_error(&source_major, &target_major).is_some() {
        anyhow::bail!(
            "restore_incompatible: cannot clone from Postgres {source_major} into older target Postgres {target_major}. Choose postgresVersion {source_major} or newer, or dump from an older-compatible source."
        );
    }

    Ok(())
}

async fn postgres_server_version_for_url(database_url: &str) -> anyhow::Result<String> {
    let (client, connection_task) = connect_url(database_url).await?;
    let version = postgres_version(&client).await;
    drop(client);
    let _ = connection_task.await;
    version
}

async fn postgres_major_for_profile(profile: &SandboxProfile) -> anyhow::Result<String> {
    if let Some(version) = &profile.postgres_version {
        return postgres_major_from_server_version(version);
    }
    let (client, connection_task) = connect_admin(profile).await?;
    let version = postgres_version(&client).await;
    drop(client);
    let _ = connection_task.await;
    postgres_major_from_server_version(&version?)
}

fn postgres_major_from_server_version(version: &str) -> anyhow::Result<String> {
    leading_digits(version).context("Postgres server_version did not start with a major version")
}

fn clone_downgrade_error(source_major: &str, target_major: &str) -> Option<String> {
    let source = source_major.parse::<u32>().ok()?;
    let target = target_major.parse::<u32>().ok()?;
    (source > target).then(|| {
        format!("source Postgres {source_major} is newer than target Postgres {target_major}")
    })
}

fn default_database_url_env() -> String {
    "DATABASE_URL".to_string()
}

fn infer_repo_postgres_version(
    repo_path: &Path,
) -> anyhow::Result<Option<RepoPostgresVersionInference>> {
    for file_name in [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ] {
        let path = repo_path.join(file_name);
        if path.is_file() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let value = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&raw)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            if let Some(inference) = find_postgres_version_in_yaml(&value, file_name, Vec::new()) {
                return Ok(Some(inference));
            }
        }
    }

    let devcontainer_path = repo_path.join(".devcontainer").join("devcontainer.json");
    if devcontainer_path.is_file() {
        let raw = fs::read_to_string(&devcontainer_path)
            .with_context(|| format!("failed to read {}", devcontainer_path.display()))?;
        let value = serde_json::from_str::<Value>(&raw)
            .with_context(|| format!("failed to parse {}", devcontainer_path.display()))?;
        if let Some(inference) =
            find_postgres_version_in_json(&value, ".devcontainer/devcontainer.json", Vec::new())
        {
            return Ok(Some(inference));
        }
    }

    Ok(None)
}

fn resolve_repo_postgres_version(
    repo_path: &Path,
    explicit: Option<String>,
) -> anyhow::Result<RepoPostgresVersionResolution> {
    if let Some(version) = explicit {
        return Ok(RepoPostgresVersionResolution {
            version: Some(version),
            source: Some("input postgresVersion".to_string()),
        });
    }

    if let Some(config) = read_repo_project_config(repo_path)? {
        if let Some(version) = config.postgres_version {
            return Ok(RepoPostgresVersionResolution {
                version: Some(version),
                source: Some(".pgsandbox/project.json postgresVersion".to_string()),
            });
        }
    }

    if let Some(inference) = infer_repo_postgres_version(repo_path)? {
        return Ok(RepoPostgresVersionResolution {
            version: Some(inference.version),
            source: Some(inference.source),
        });
    }

    Ok(RepoPostgresVersionResolution {
        version: None,
        source: None,
    })
}

fn find_postgres_version_in_yaml(
    value: &serde_yaml_ng::Value,
    file_name: &str,
    path: Vec<String>,
) -> Option<RepoPostgresVersionInference> {
    match value {
        serde_yaml_ng::Value::Mapping(mapping) => {
            for (key, child) in mapping {
                let Some(key) = key.as_str() else {
                    continue;
                };
                let mut child_path = path.clone();
                child_path.push(key.to_string());
                if key == "image" {
                    if let Some(image) = child.as_str() {
                        if let Some(version) = postgres_version_from_image(image) {
                            return Some(RepoPostgresVersionInference {
                                version,
                                source: format!("{file_name} {}", child_path.join(".")),
                            });
                        }
                    }
                }
                if let Some(inference) = find_postgres_version_in_yaml(child, file_name, child_path)
                {
                    return Some(inference);
                }
            }
            None
        }
        serde_yaml_ng::Value::Sequence(items) => {
            for (index, child) in items.iter().enumerate() {
                let mut child_path = path.clone();
                child_path.push(index.to_string());
                if let Some(inference) = find_postgres_version_in_yaml(child, file_name, child_path)
                {
                    return Some(inference);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_postgres_version_in_json(
    value: &Value,
    file_name: &str,
    path: Vec<String>,
) -> Option<RepoPostgresVersionInference> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let mut child_path = path.clone();
                child_path.push(key.to_string());
                if key == "image" {
                    if let Some(image) = child.as_str() {
                        if let Some(version) = postgres_version_from_image(image) {
                            return Some(RepoPostgresVersionInference {
                                version,
                                source: format!("{file_name} {}", child_path.join(".")),
                            });
                        }
                    }
                }
                if let Some(inference) = find_postgres_version_in_json(child, file_name, child_path)
                {
                    return Some(inference);
                }
            }
            None
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let mut child_path = path.clone();
                child_path.push(index.to_string());
                if let Some(inference) = find_postgres_version_in_json(child, file_name, child_path)
                {
                    return Some(inference);
                }
            }
            None
        }
        _ => None,
    }
}

fn postgres_version_from_image(image: &str) -> Option<String> {
    let image = image.trim().to_ascii_lowercase();
    let (name, tag) = docker_image_name_and_tag(&image)?;
    if !is_postgres_image_name(name) {
        return None;
    }
    postgres_major_from_tag(tag)
}

fn docker_image_name_and_tag(image: &str) -> Option<(&str, &str)> {
    let image = image.split_once('@').map_or(image, |(image, _)| image);
    let tag_separator = image.rfind(':')?;
    let last_slash = image.rfind('/');
    if last_slash.is_some_and(|slash| tag_separator < slash) {
        return None;
    }
    let name = &image[..tag_separator];
    let tag = &image[tag_separator + 1..];
    (!name.is_empty() && !tag.is_empty()).then_some((name, tag))
}

fn is_postgres_image_name(name: &str) -> bool {
    let mut parts = name.split('/').collect::<Vec<_>>();
    if parts.len() > 1
        && parts
            .first()
            .is_some_and(|part| part.contains('.') || part.contains(':') || *part == "localhost")
    {
        parts.remove(0);
    }
    let repository = parts.join("/");
    matches!(
        repository.as_str(),
        "postgres"
            | "library/postgres"
            | "postgis/postgis"
            | "timescale/timescaledb"
            | "timescaledb/timescaledb"
    )
}

fn postgres_major_from_tag(tag: &str) -> Option<String> {
    tag.split(['-', '_', '.'])
        .find_map(|part| part.strip_prefix("pg").and_then(leading_digits))
        .or_else(|| leading_digits(tag))
}

fn leading_digits(value: &str) -> Option<String> {
    let digits = value
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty()).then_some(digits)
}

fn repo_project_config_path(repo_path: &Path) -> PathBuf {
    repo_path.join(".pgsandbox").join("project.json")
}

fn write_repo_project_config(
    repo_path: &Path,
    config: &RepoProjectConfig,
) -> anyhow::Result<PathBuf> {
    let path = repo_project_config_path(repo_path);
    write_json_file(&path, config)?;
    Ok(path)
}

fn read_repo_project_config(repo_path: &Path) -> anyhow::Result<Option<RepoProjectConfig>> {
    read_json_file(&repo_project_config_path(repo_path))
}

fn resolve_repo_schema_command(
    repo_path: &Path,
    input_command: Option<Vec<String>>,
) -> anyhow::Result<Result<Vec<String>, WorkflowError>> {
    let command = match input_command {
        Some(command) => command,
        None => match read_repo_project_config(repo_path)? {
            Some(config) => match config.migration_command {
                Some(command) => command,
                None => {
                    return Ok(Err(workflow_error(
                        "missing_schema_change_command",
                        ".pgsandbox/project.json has no migrationCommand.",
                        Some("Pass an explicit repo command argv array or add migrationCommand to the project config.".to_string()),
                    )))
                }
            },
            None => {
                return Ok(Err(workflow_error(
                    "missing_schema_change_command",
                    "No schema change command was provided and .pgsandbox/project.json is missing.",
                    Some(
                        "Pass an explicit repo command argv array, call run_sql for direct SQL, or run prepare_for_repo with migrationCommand."
                            .to_string(),
                    ),
                )))
            }
        },
    };
    if let Err(error) = validate_workflow_command(&command, "Migration command") {
        return Ok(Err(error));
    }
    Ok(Ok(command))
}

fn resolve_seed_command(
    repo_path: &Path,
    input_command: Option<Vec<String>>,
) -> anyhow::Result<Result<Vec<String>, WorkflowError>> {
    let command = match input_command {
        Some(command) => command,
        None => match read_repo_project_config(repo_path)? {
            Some(config) => match config.seed_command {
                Some(command) => command,
                None => {
                    return Ok(Err(workflow_error(
                        "missing_seed_command",
                        "No seed command was provided and .pgsandbox/project.json has no seedCommand.",
                        Some("Pass an explicit seed command argv array or add seedCommand to the project config.".to_string()),
                    )))
                }
            },
            None => {
                return Ok(Err(workflow_error(
                    "missing_seed_command",
                    "No seed command was provided and .pgsandbox/project.json is missing.",
                    Some("Pass an explicit seed command argv array or run prepare_for_repo and add seedCommand.".to_string()),
                )))
            }
        },
    };
    if let Err(error) = validate_workflow_command(&command, "Seed command") {
        return Ok(Err(error));
    }
    Ok(Ok(command))
}

fn validate_workflow_command(command: &[String], label: &str) -> Result<(), WorkflowError> {
    if command.is_empty() {
        return Err(workflow_command_bounds_error(command, label));
    }
    if command_invokes_shell(command) {
        return Err(workflow_error(
            "unsafe_command",
            format!("{label} cannot invoke a shell or command launcher."),
            Some("Shells such as bash -lc and indirect launchers such as env/sudo are intentionally unsupported. Pass the executable and arguments directly, for example [\"npm\", \"run\", \"migrate\"] or [\"alembic\", \"upgrade\", \"head\"].".to_string()),
        ));
    }
    if !command_is_bounded(command) {
        return Err(workflow_command_bounds_error(command, label));
    }
    Ok(())
}

fn workflow_command_bounds_error(command: &[String], label: &str) -> WorkflowError {
    let part_count = command.len();
    let total_len = command.iter().map(String::len).sum::<usize>();
    let longest_part_len = command.iter().map(String::len).max().unwrap_or(0);
    let part_suffix = if part_count == 1 { "part" } else { "parts" };
    let longest_clause = if part_count > 0 {
        format!(", and longest part is {longest_part_len} bytes")
    } else {
        String::new()
    };
    workflow_error(
        "unclear_command",
        format!(
            "{label} is invalid: observed {part_count} argv {part_suffix}, {total_len} total bytes{longest_clause}. Limits: 1 to maximum {MAX_WORKFLOW_COMMAND_PARTS} argv parts, maximum {MAX_WORKFLOW_COMMAND_TOTAL_BYTES} total bytes, maximum {MAX_WORKFLOW_COMMAND_PART_BYTES} bytes per part, and no NUL/newline characters."
        ),
        Some(
            format!(
                "Pass a direct argv array with 1 to {MAX_WORKFLOW_COMMAND_PARTS} parts, maximum {MAX_WORKFLOW_COMMAND_TOTAL_BYTES} total bytes, maximum {MAX_WORKFLOW_COMMAND_PART_BYTES} bytes per part, and no NUL/newline characters. Commands are executed without shell expansion."
            ),
        ),
    )
}

fn command_is_bounded(command: &[String]) -> bool {
    if command.is_empty() || command.len() > MAX_WORKFLOW_COMMAND_PARTS {
        return false;
    }
    let total_len = command.iter().map(String::len).sum::<usize>();
    total_len <= MAX_WORKFLOW_COMMAND_TOTAL_BYTES
        && command.iter().all(|part| {
            !part.is_empty()
                && part.len() <= MAX_WORKFLOW_COMMAND_PART_BYTES
                && !part.contains('\0')
                && !part.contains('\n')
                && !part.contains('\r')
        })
}

fn command_invokes_shell(command: &[String]) -> bool {
    command.iter().any(|part| command_part_is_shell(part))
        || command
            .first()
            .is_some_and(|program| command_part_is_indirect_launcher(program))
}

fn command_part_executable_name(part: &str) -> String {
    Path::new(part)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(part)
        .to_ascii_lowercase()
}

fn command_part_is_shell(part: &str) -> bool {
    let executable = command_part_executable_name(part);
    matches!(
        executable.as_str(),
        "sh" | "bash"
            | "dash"
            | "zsh"
            | "fish"
            | "ksh"
            | "csh"
            | "tcsh"
            | "cmd"
            | "cmd.exe"
            | "powershell"
            | "powershell.exe"
            | "pwsh"
            | "pwsh.exe"
    )
}

fn command_part_is_indirect_launcher(part: &str) -> bool {
    let executable = command_part_executable_name(part);
    matches!(
        executable.as_str(),
        "env"
            | "sudo"
            | "sudoedit"
            | "doas"
            | "su"
            | "runuser"
            | "xargs"
            | "nsenter"
            | "unshare"
            | "chroot"
            | "setsid"
            | "nohup"
            | "nice"
            | "stdbuf"
    )
}

fn workflow_timeout(timeout_seconds: Option<u64>) -> StdDuration {
    StdDuration::from_secs(
        timeout_seconds
            .unwrap_or(DEFAULT_WORKFLOW_TIMEOUT_SECONDS)
            .min(MAX_WORKFLOW_TIMEOUT_SECONDS),
    )
}

async fn execute_repo_command(
    repo_path: &Path,
    command: &[String],
    database_url: &str,
    timeout: StdDuration,
) -> anyhow::Result<CommandRunResult> {
    if !repo_path.is_dir() {
        anyhow::bail!("repoPath is not a directory: {}", repo_path.display());
    }
    if !command_is_bounded(command) {
        anyhow::bail!("command is empty or too large");
    }
    let env = database_command_env(database_url)?;
    let started = std::time::Instant::now();
    let mut command_builder = Command::new(&command[0]);
    apply_command_env(&mut command_builder, &env);
    configure_repo_command_process(&mut command_builder);
    let mut child = command_builder
        .args(&command[1..])
        .current_dir(repo_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to start command `{}`", command[0]))?;
    let stdout = child
        .stdout
        .take()
        .context("stdout pipe was not available")?;
    let stderr = child
        .stderr
        .take()
        .context("stderr pipe was not available")?;
    let stdout_task = tokio::spawn(read_bounded_output(stdout));
    let stderr_task = tokio::spawn(read_bounded_output(stderr));
    let status = match time::timeout(timeout, child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            terminate_repo_command(&mut child).await;
            let (stdout, stdout_truncated) = stdout_task.await.context("stdout task failed")??;
            let (stderr, stderr_truncated) = stderr_task.await.context("stderr task failed")??;
            return Ok(CommandRunResult {
                command: command.to_vec(),
                elapsed_ms: started.elapsed().as_millis(),
                exit_code: None,
                stdout,
                stderr: append_timeout_message(stderr, timeout),
                stdout_truncated,
                stderr_truncated,
            });
        }
    };
    let (stdout, stdout_truncated) = stdout_task.await.context("stdout task failed")??;
    let (stderr, stderr_truncated) = stderr_task.await.context("stderr task failed")??;
    Ok(CommandRunResult {
        command: command.to_vec(),
        elapsed_ms: started.elapsed().as_millis(),
        exit_code: status.code(),
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

fn configure_repo_command_process(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

async fn terminate_repo_command(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(process_group_id) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
        // Negating the spawned child pid targets the process group created above.
        unsafe {
            libc::kill(-process_group_id, libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

fn apply_command_env(command: &mut Command, env: &BTreeMap<String, String>) {
    command.env_clear().envs(env.iter());
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
}

async fn read_bounded_output<R>(mut reader: R) -> anyhow::Result<(String, bool)>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 4096];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let remaining = MAX_COMMAND_OUTPUT_BYTES.saturating_sub(output.len());
        if remaining > 0 {
            let take = remaining.min(count);
            output.extend_from_slice(&buffer[..take]);
            if take < count {
                truncated = true;
                break;
            }
        } else {
            truncated = true;
            break;
        }
    }
    Ok((String::from_utf8_lossy(&output).to_string(), truncated))
}

fn append_timeout_message(mut stderr: String, timeout: StdDuration) -> String {
    if !stderr.ends_with('\n') && !stderr.is_empty() {
        stderr.push('\n');
    }
    stderr.push_str(&format!(
        "PGSandbox command timed out after {} seconds.",
        timeout.as_secs()
    ));
    stderr
}

fn command_workflow_output(
    database_id: &str,
    database_name: &str,
    result: CommandRunResult,
) -> CommandWorkflowOutput {
    CommandWorkflowOutput {
        database_id: database_id.to_string(),
        database_name: database_name.to_string(),
        command: result.command,
        elapsed_ms: result.elapsed_ms,
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
    }
}

fn validate_schema_change_output(
    database_id: &str,
    database_name: &str,
    created_sandbox: bool,
    result: CommandRunResult,
    schema_diff: WorkflowSchemaDiffOutput,
) -> ValidateSchemaChangeOutput {
    ValidateSchemaChangeOutput {
        database_id: database_id.to_string(),
        database_name: database_name.to_string(),
        created_sandbox,
        command: result.command,
        elapsed_ms: result.elapsed_ms,
        exit_code: result.exit_code,
        schema_diff,
        stdout: result.stdout,
        stderr: result.stderr,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
    }
}

fn database_command_env(database_url: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let connection = pg_tool_connection_from_url(database_url)?;
    let mut env = connection.env;
    env.insert("PGDATABASE".to_string(), connection.database);
    env.insert("DATABASE_URL".to_string(), database_url.to_string());
    env.insert(
        "PGSANDBOX_DATABASE_URL".to_string(),
        database_url.to_string(),
    );
    Ok(env)
}

fn pgsandbox_state_root() -> anyhow::Result<PathBuf> {
    match std::env::var_os("PGSANDBOX_HOME") {
        Some(path) => Ok(PathBuf::from(path)),
        None => Ok(dirs::home_dir()
            .context("could not resolve home directory for ~/.pgsandbox")?
            .join(".pgsandbox")),
    }
}

fn validate_artifact_name(value: &str, field: &str) -> Result<String, WorkflowError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(workflow_error(
            "invalid_artifact_name",
            format!("{field} cannot be empty."),
            Some("Use letters, numbers, dots, underscores, or hyphens.".to_string()),
        ));
    }
    if trimmed.len() > 80
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || !trimmed.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
    {
        return Err(workflow_error(
            "invalid_artifact_name",
            format!("{field} must be 1-80 safe filename characters."),
            Some(
                "Use letters, numbers, dots, underscores, or hyphens; do not include paths."
                    .to_string(),
            ),
        ));
    }
    Ok(trimmed.to_string())
}

fn profile_artifact_component(profile: &str) -> String {
    validate_artifact_name(profile, "profile")
        .unwrap_or_else(|_| slugify_profile_component(profile))
}

fn slugify_profile_component(profile: &str) -> String {
    let slug = profile
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if slug.is_empty() {
        "profile".to_string()
    } else {
        slug
    }
}

fn template_paths(profile: &str, template_name: &str) -> anyhow::Result<TemplatePaths> {
    let profile = profile_artifact_component(profile);
    let dir = pgsandbox_state_root()?.join("templates").join(profile);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create template directory {}", dir.display()))?;
    Ok(TemplatePaths {
        dump_path: dir.join(format!("{template_name}.dump")),
        metadata_path: dir.join(format!("{template_name}.json")),
    })
}

fn snapshot_paths(
    profile: &str,
    database_id: &str,
    snapshot_name: &str,
) -> anyhow::Result<SnapshotPaths> {
    let profile = profile_artifact_component(profile);
    let database_id = validate_artifact_name(database_id, "databaseId").map_err(|error| {
        anyhow::anyhow!(
            "invalid databaseId for schema snapshot path: {}",
            error.message
        )
    })?;
    let dir = pgsandbox_state_root()?
        .join("schema-snapshots")
        .join(profile)
        .join(database_id);
    fs::create_dir_all(&dir).with_context(|| {
        format!(
            "failed to create schema snapshot directory {}",
            dir.display()
        )
    })?;
    Ok(SnapshotPaths {
        metadata_path: dir.join(format!("{snapshot_name}.json")),
    })
}

fn read_schema_snapshot(
    profile: &str,
    database_id: &str,
    snapshot_name: &str,
) -> anyhow::Result<Option<SchemaSnapshotRecord>> {
    let paths = snapshot_paths(profile, database_id, snapshot_name)?;
    read_json_file(&paths.metadata_path)
}

fn read_schema_snapshots(
    profile: &str,
    database_id: &str,
) -> anyhow::Result<Vec<SchemaSnapshotRecord>> {
    let dir = pgsandbox_state_root()?
        .join("schema-snapshots")
        .join(profile_artifact_component(profile))
        .join(database_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            if let Some(snapshot) = read_json_file::<SchemaSnapshotRecord>(&path)? {
                snapshots.push(snapshot);
            }
        }
    }
    snapshots.sort_by(|left, right| left.snapshot_name.cmp(&right.snapshot_name));
    Ok(snapshots)
}

fn snapshot_summary(snapshot: &SchemaSnapshotRecord) -> SchemaSnapshotSummary {
    SchemaSnapshotSummary {
        snapshot_name: snapshot.snapshot_name.clone(),
        profile: snapshot.profile.clone(),
        database_id: snapshot.database_id.clone(),
        database_name: snapshot.database_name.clone(),
        created_at: snapshot.created_at,
        postgres_version: snapshot.postgres_version.clone(),
        digest_version: snapshot.digest_version,
        object_counts: snapshot.object_counts.clone(),
        notes: snapshot.notes.clone(),
    }
}

fn snapshot_detail_handle(profile: &str, database_id: &str, snapshot_name: &str) -> Value {
    json!({
        "type": "schema-snapshot",
        "profile": profile,
        "databaseId": database_id,
        "snapshotName": snapshot_name
    })
}

fn template_detail_handle(profile: &str, template_name: &str) -> Value {
    json!({
        "type": "template",
        "profile": profile,
        "templateName": template_name
    })
}

fn read_templates(profile: &str) -> anyhow::Result<Vec<TemplateMetadata>> {
    let dir = pgsandbox_state_root()?
        .join("templates")
        .join(profile_artifact_component(profile));
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut templates = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            if let Some(metadata) = read_json_file::<TemplateMetadata>(&path)? {
                templates.push(metadata);
            }
        }
    }
    templates.sort_by(|left, right| left.template_name.cmp(&right.template_name));
    Ok(templates)
}

fn write_json_file<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let raw = format!("{}\n", serde_json::to_string_pretty(value)?);
    fs::write(path, raw).with_context(|| format!("failed to write {}", path.display()))
}

fn read_json_file<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse JSON {}", path.display()))
        .map(Some)
}

fn remove_file_if_exists(path: &Path) -> anyhow::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

async fn dump_database_to_file(database_url: &str, dump_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = dump_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create template directory {}", parent.display()))?;
    }
    let temp_path = dump_path.with_extension(format!("dump.tmp-{}", Uuid::new_v4().simple()));
    let connection = pg_tool_connection_from_url(database_url)
        .context("sandbox connection string is not a supported Postgres URL")?;
    let mut command = Command::new("pg_dump");
    apply_command_env(&mut command, &connection.env);
    let output = command
        .args(pg_dump_file_args(&connection.database, &temp_path, false))
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to start pg_dump; install PostgreSQL client tools and ensure pg_dump is on PATH")?;
    if !output.status.success() {
        let _ = fs::remove_file(&temp_path);
        anyhow::bail!("pg_dump failed: {}", summarize_tool_stderr(&output.stderr));
    }
    if dump_path.exists() {
        fs::remove_file(dump_path)
            .with_context(|| format!("failed to replace template dump {}", dump_path.display()))?;
    }
    fs::rename(&temp_path, dump_path).with_context(|| {
        format!(
            "failed to move template dump from {} to {}",
            temp_path.display(),
            dump_path.display()
        )
    })?;
    Ok(())
}

async fn restore_database_from_file(dump_path: &Path, database_url: &str) -> anyhow::Result<()> {
    let connection = pg_tool_connection_from_url(database_url)
        .context("target sandbox connection string is not a supported Postgres URL")?;
    let mut command = Command::new("pg_restore");
    apply_command_env(&mut command, &connection.env);
    let output = command
        .args(pg_restore_file_args(&connection.database, dump_path))
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to start pg_restore; install PostgreSQL client tools and ensure pg_restore is on PATH")?;
    if !output.status.success() {
        anyhow::bail!(
            "pg_restore failed: {}",
            summarize_tool_stderr(&output.stderr)
        );
    }
    Ok(())
}

fn pg_dump_file_args(database: &str, output_file: &Path, schema_only: bool) -> Vec<String> {
    let mut args = pg_dump_args(database, schema_only);
    args.extend(["--file".to_string(), output_file.display().to_string()]);
    args
}

fn pg_restore_file_args(database: &str, dump_file: &Path) -> Vec<String> {
    let mut args = pg_restore_args(database);
    args.push(dump_file.display().to_string());
    args
}

fn mask_connection_string(value: &str) -> String {
    if let Ok(mut url) = Url::parse(value) {
        if url.password().is_some() {
            let _ = url.set_password(Some("****"));
        }
        return url.to_string();
    }
    "<unparseable connection string>".to_string()
}

async fn connect_admin(
    profile: &SandboxProfile,
) -> anyhow::Result<(
    Client,
    tokio::task::JoinHandle<Result<(), tokio_postgres::Error>>,
)> {
    connect_url(&profile.admin_url).await.with_context(|| {
        format!(
            "failed to connect to Postgres admin profile {} at {}",
            profile.name,
            mask_connection_string(&profile.admin_url)
        )
    })
}

pub(crate) async fn connect_url(
    url: &str,
) -> anyhow::Result<(
    Client,
    tokio::task::JoinHandle<Result<(), tokio_postgres::Error>>,
)> {
    match ssl_mode_from_url(url)? {
        SslMode::Disable => connect_url_no_tls(url).await,
        SslMode::Allow => match connect_url_no_tls(url).await {
            Ok(connection) => Ok(connection),
            Err(no_tls_error) => connect_url_with_tls(url, TlsVerification::VerifyFull)
                .await
                .with_context(|| format!("plaintext connection failed: {no_tls_error}")),
        },
        SslMode::Prefer => match connect_url_with_tls(url, TlsVerification::VerifyFull).await {
            Ok(connection) => Ok(connection),
            Err(tls_error) => connect_url_no_tls(url)
                .await
                .with_context(|| format!("TLS connection failed: {tls_error}")),
        },
        SslMode::Require => connect_url_with_tls(url, TlsVerification::Unverified).await,
        SslMode::VerifyCa => connect_url_with_tls(url, TlsVerification::VerifyCa).await,
        SslMode::VerifyFull => connect_url_with_tls(url, TlsVerification::VerifyFull).await,
    }
}

async fn connect_url_with_tls(
    url: &str,
    verification: TlsVerification,
) -> anyhow::Result<(
    Client,
    tokio::task::JoinHandle<Result<(), tokio_postgres::Error>>,
)> {
    let tls = MakeTlsConnector::new(tls_connector(verification)?);
    let (client, connection) = tokio_postgres::connect(url, tls).await?;
    let task = tokio::spawn(connection);
    Ok((client, task))
}

async fn connect_url_no_tls(
    url: &str,
) -> anyhow::Result<(
    Client,
    tokio::task::JoinHandle<Result<(), tokio_postgres::Error>>,
)> {
    let (client, connection) = tokio_postgres::connect(url, NoTls).await?;
    let task = tokio::spawn(connection);
    Ok((client, task))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SslMode {
    Disable,
    Allow,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TlsVerification {
    Unverified,
    VerifyCa,
    VerifyFull,
}

fn ssl_mode_from_url(url: &str) -> anyhow::Result<SslMode> {
    let parsed = Url::parse(url).context("Postgres URL is invalid")?;
    let sslmode = parsed
        .query_pairs()
        .find(|(key, _)| key.eq_ignore_ascii_case("sslmode"))
        .map(|(_, value)| value.to_ascii_lowercase());

    match sslmode.as_deref().unwrap_or("prefer") {
        "disable" => Ok(SslMode::Disable),
        "allow" => Ok(SslMode::Allow),
        "prefer" => Ok(SslMode::Prefer),
        "require" => Ok(SslMode::Require),
        "verify-ca" => Ok(SslMode::VerifyCa),
        "verify-full" => Ok(SslMode::VerifyFull),
        value => anyhow::bail!("unsupported Postgres sslmode: {value}"),
    }
}

fn tls_connector(verification: TlsVerification) -> anyhow::Result<native_tls::TlsConnector> {
    let mut builder = native_tls::TlsConnector::builder();
    match verification {
        TlsVerification::Unverified => {
            builder.danger_accept_invalid_certs(true);
            builder.danger_accept_invalid_hostnames(true);
        }
        TlsVerification::VerifyCa => {
            builder.danger_accept_invalid_hostnames(true);
        }
        TlsVerification::VerifyFull => {}
    }
    Ok(builder.build()?)
}

async fn ensure_metadata_table(client: &Client) -> anyhow::Result<()> {
    client
        .batch_execute(&format!(
            r#"
              CREATE TABLE IF NOT EXISTS {} (
                database_id text PRIMARY KEY,
                profile_name text NOT NULL,
                database_name text NOT NULL UNIQUE,
                role_name text NOT NULL UNIQUE,
                role_password text NOT NULL,
                owner text,
                purpose text,
                labels jsonb NOT NULL DEFAULT '{{}}'::jsonb,
                created_at timestamptz NOT NULL DEFAULT now(),
                expires_at timestamptz NOT NULL,
                deleted_at timestamptz
              )
            "#,
            quote_ident(METADATA_TABLE)?
        ))
        .await?;
    client
        .batch_execute(&format!(
            r#"
              CREATE TABLE IF NOT EXISTS {} (
                event_id text PRIMARY KEY,
                profile_name text NOT NULL,
                database_id text NOT NULL,
                database_name text NOT NULL,
                role_name text,
                event_type text NOT NULL,
                details jsonb NOT NULL DEFAULT '{{}}'::jsonb,
                created_at timestamptz NOT NULL DEFAULT now()
              )
            "#,
            quote_ident(AUDIT_TABLE)?
        ))
        .await?;
    Ok(())
}

async fn record_audit_event(
    client: &Client,
    event_type: &str,
    profile_name: &str,
    database_id: &str,
    database_name: &str,
    role_name: Option<&str>,
    details: Value,
) -> anyhow::Result<()> {
    client
        .execute(
            &format!(
                r#"
                  INSERT INTO {}
                    (event_id, profile_name, database_id, database_name, role_name, event_type, details)
                  VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb)
                "#,
                quote_ident(AUDIT_TABLE)?
            ),
            &[
                &Uuid::new_v4().to_string() as &(dyn ToSql + Sync),
                &profile_name,
                &database_id,
                &database_name,
                &role_name,
                &event_type,
                &details,
            ],
        )
        .await?;
    Ok(())
}

async fn enforce_owner_quota(
    client: &Client,
    profile: &SandboxProfile,
    owner: Option<&str>,
) -> anyhow::Result<()> {
    let Some(limit) = profile.max_active_databases_per_owner else {
        return Ok(());
    };
    let Some(owner) = owner.filter(|owner| !owner.trim().is_empty()) else {
        return Ok(());
    };

    let row = client
        .query_one(&active_owner_quota_sql()?, &[&profile.name, &owner])
        .await?;
    let active_count = row.get::<_, i64>("active_count");
    if active_count >= i64::from(limit) {
        anyhow::bail!(
            "owner {owner} already has {active_count} active sandbox database(s), which meets maxActiveDatabasesPerOwner ({limit}) for profile {}",
            profile.name
        );
    }

    Ok(())
}

fn active_owner_quota_sql() -> anyhow::Result<String> {
    Ok(format!(
        r#"
          SELECT count(*)::bigint AS active_count
          FROM {}
          WHERE profile_name = $1
            AND owner = $2
            AND deleted_at IS NULL
            AND expires_at > now()
        "#,
        quote_ident(METADATA_TABLE)?
    ))
}

async fn find_record(
    client: &Client,
    profile_name: &str,
    input: &DatabaseSelector,
) -> anyhow::Result<Option<SandboxRecord>> {
    if input.database_id.is_none() && input.database_name.is_none() {
        anyhow::bail!("Provide databaseId or databaseName.");
    }

    let rows = client
        .query(
            &format!(
                r#"
                  SELECT *
                  FROM {}
                  WHERE deleted_at IS NULL
                    AND profile_name = $3
                    AND (($1::text IS NOT NULL AND database_id = $1)
                      OR ($2::text IS NOT NULL AND database_name = $2))
                  LIMIT 1
                "#,
                quote_ident(METADATA_TABLE)?
            ),
            &[&input.database_id, &input.database_name, &profile_name],
        )
        .await?;

    Ok(rows.first().map(sandbox_record_from_row))
}

async fn terminate_database_connections(
    client: &Client,
    database_name: &str,
) -> anyhow::Result<()> {
    client
        .execute(
            r#"
              SELECT pg_terminate_backend(pid)
              FROM pg_stat_activity
              WHERE datname = $1
                AND pid <> pg_backend_pid()
            "#,
            &[&database_name],
        )
        .await?;
    Ok(())
}

async fn schema_digest_for_connection(
    client: &Client,
    database_id: String,
    database_name: String,
) -> anyhow::Result<SchemaDigestOutput> {
    let table_rows = client
        .query(
            r#"
              SELECT n.nspname AS table_schema,
                     c.relname AS table_name,
                     CASE c.relkind
                       WHEN 'r' THEN 'table'
                       WHEN 'p' THEN 'partitioned_table'
                       WHEN 'v' THEN 'view'
                       WHEN 'm' THEN 'materialized_view'
                       WHEN 'f' THEN 'foreign_table'
                       ELSE c.relkind::text
                     END AS relation_kind,
                     CASE WHEN c.relkind IN ('v', 'm') THEN pg_get_viewdef(c.oid, true) ELSE NULL END AS view_definition
              FROM pg_class c
              JOIN pg_namespace n ON n.oid = c.relnamespace
              WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f')
                AND n.nspname NOT IN ('pg_catalog', 'information_schema')
              ORDER BY n.nspname, c.relname
            "#,
            &[],
        )
        .await?;
    let column_rows = client
        .query(
            r#"
              SELECT n.nspname AS table_schema,
                     c.relname AS table_name,
                     a.attname AS column_name,
                     pg_catalog.format_type(a.atttypid, a.atttypmod) AS data_type,
                     NOT a.attnotnull AS nullable,
                     CASE WHEN a.attgenerated = '' THEN pg_get_expr(ad.adbin, ad.adrelid) ELSE NULL END AS default_expression,
                     CASE WHEN a.attgenerated = '' THEN NULL ELSE pg_get_expr(ad.adbin, ad.adrelid) END AS generated_expression
              FROM pg_attribute a
              JOIN pg_class c ON c.oid = a.attrelid
              JOIN pg_namespace n ON n.oid = c.relnamespace
              LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum
              WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f')
                AND a.attnum > 0
                AND NOT a.attisdropped
                AND n.nspname NOT IN ('pg_catalog', 'information_schema')
              ORDER BY n.nspname, c.relname, a.attnum
            "#,
            &[],
        )
        .await?;
    let constraint_rows = client
        .query(
            r#"
              SELECT n.nspname AS table_schema,
                     c.relname AS table_name,
                     con.conname AS constraint_name,
                     CASE con.contype
                       WHEN 'p' THEN 'primary_key'
                       WHEN 'u' THEN 'unique'
                       WHEN 'f' THEN 'foreign_key'
                       WHEN 'c' THEN 'check'
                       WHEN 'x' THEN 'exclusion'
                       ELSE con.contype::text
                     END AS constraint_type,
                     pg_get_constraintdef(con.oid, true) AS definition,
                     CASE con.confupdtype
                       WHEN 'a' THEN 'no_action'
                       WHEN 'r' THEN 'restrict'
                       WHEN 'c' THEN 'cascade'
                       WHEN 'n' THEN 'set_null'
                       WHEN 'd' THEN 'set_default'
                       ELSE NULL
                     END AS update_action,
                     CASE con.confdeltype
                       WHEN 'a' THEN 'no_action'
                       WHEN 'r' THEN 'restrict'
                       WHEN 'c' THEN 'cascade'
                       WHEN 'n' THEN 'set_null'
                       WHEN 'd' THEN 'set_default'
                       ELSE NULL
                     END AS delete_action
              FROM pg_constraint con
              JOIN pg_class c ON c.oid = con.conrelid
              JOIN pg_namespace n ON n.oid = c.relnamespace
              WHERE c.relkind IN ('r', 'p')
                AND n.nspname NOT IN ('pg_catalog', 'information_schema')
              ORDER BY n.nspname, c.relname, con.conname
            "#,
            &[],
        )
        .await?;
    let index_rows = client
        .query(
            r#"
              SELECT schemaname, tablename, indexname, indexdef
              FROM pg_indexes
              WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
              ORDER BY schemaname, tablename, indexname
            "#,
            &[],
        )
        .await?;
    let extension_rows = client
        .query(
            "SELECT extname, extversion FROM pg_extension ORDER BY extname",
            &[],
        )
        .await?;

    let mut tables = BTreeMap::<(String, String), SchemaDigestTable>::new();
    for row in table_rows {
        let schema = row.get::<_, String>("table_schema");
        let name = row.get::<_, String>("table_name");
        let view_definition = row.get::<_, Option<String>>("view_definition");
        tables.insert(
            (schema.clone(), name.clone()),
            SchemaDigestTable {
                schema,
                name,
                relation_kind: row.get("relation_kind"),
                columns: Vec::new(),
                constraints: Vec::new(),
                indexes: Vec::new(),
                view_definition_hash: view_definition
                    .as_deref()
                    .map(|definition| sha256_hex(definition.as_bytes())),
            },
        );
    }

    for row in column_rows {
        let schema = row.get::<_, String>("table_schema");
        let name = row.get::<_, String>("table_name");
        let table = tables
            .entry((schema.clone(), name.clone()))
            .or_insert_with(|| SchemaDigestTable {
                schema,
                name,
                relation_kind: "table".to_string(),
                columns: Vec::new(),
                constraints: Vec::new(),
                indexes: Vec::new(),
                view_definition_hash: None,
            });
        table.columns.push(SchemaDigestColumn {
            name: row.get("column_name"),
            data_type: row.get("data_type"),
            nullable: row.get("nullable"),
            default_expression: row.get("default_expression"),
            generated_expression: row.get("generated_expression"),
        });
    }

    for row in constraint_rows {
        let schema = row.get::<_, String>("table_schema");
        let name = row.get::<_, String>("table_name");
        let definition = row.get::<_, String>("definition");
        let table = tables
            .entry((schema.clone(), name.clone()))
            .or_insert_with(|| SchemaDigestTable {
                schema,
                name,
                relation_kind: "table".to_string(),
                columns: Vec::new(),
                constraints: Vec::new(),
                indexes: Vec::new(),
                view_definition_hash: None,
            });
        table.constraints.push(SchemaDigestConstraint {
            name: row.get("constraint_name"),
            constraint_type: row.get("constraint_type"),
            definition_hash: sha256_hex(definition.as_bytes()),
            update_action: row.get("update_action"),
            delete_action: row.get("delete_action"),
        });
    }

    for row in index_rows {
        let schema = row.get::<_, String>("schemaname");
        let name = row.get::<_, String>("tablename");
        let indexdef = row.get::<_, String>("indexdef");
        if let Some(table) = tables.get_mut(&(schema, name)) {
            table.indexes.push(SchemaDigestIndex {
                name: row.get("indexname"),
                definition_hash: sha256_hex(indexdef.as_bytes()),
            });
        }
    }

    let tables = tables.into_values().collect::<Vec<_>>();
    let relation_counts = relation_counts_for_digest_tables(&tables);
    let extensions = extension_rows
        .into_iter()
        .map(|row| SchemaDigestExtension {
            name: row.get("extname"),
            version: row.get("extversion"),
        })
        .collect::<Vec<_>>();

    let table_count = relation_counts.tables + relation_counts.partitioned_tables;
    let column_count = tables.iter().map(|table| table.columns.len()).sum();
    let constraint_count = tables.iter().map(|table| table.constraints.len()).sum();
    let index_count = tables.iter().map(|table| table.indexes.len()).sum();
    let extension_count = extensions.len();
    let checksum = schema_digest_checksum(&tables, &extensions)?;

    Ok(SchemaDigestOutput {
        database_id,
        database_name,
        digest_version: SCHEMA_DIGEST_VERSION,
        checksum,
        table_count,
        relation_counts,
        column_count,
        constraint_count,
        index_count,
        extension_count,
        tables,
        extensions,
    })
}

fn schema_digest_checksum(
    tables: &[SchemaDigestTable],
    extensions: &[SchemaDigestExtension],
) -> anyhow::Result<String> {
    let content = SchemaDigestContent {
        digest_version: SCHEMA_DIGEST_VERSION,
        tables,
        extensions,
    };
    Ok(sha256_hex(&serde_json::to_vec(&content)?))
}

fn default_relation_kind() -> String {
    "table".to_string()
}

fn relation_counts_from_rows(rows: &[Row]) -> SchemaRelationCounts {
    let mut counts = SchemaRelationCounts::default();
    for row in rows {
        increment_relation_count(&mut counts, row.get::<_, String>("relation_kind").as_str());
    }
    counts
}

fn relation_counts_for_digest_tables(tables: &[SchemaDigestTable]) -> SchemaRelationCounts {
    let mut counts = SchemaRelationCounts::default();
    for table in tables {
        increment_relation_count(&mut counts, &table.relation_kind);
    }
    counts
}

fn relation_counts_for_schema_objects(objects: &[SchemaObjectDigest]) -> SchemaRelationCounts {
    let mut counts = SchemaRelationCounts::default();
    for object in objects {
        increment_relation_count(&mut counts, &object.kind);
    }
    counts
}

fn increment_relation_count(counts: &mut SchemaRelationCounts, relation_kind: &str) {
    match relation_kind {
        "table" => counts.tables += 1,
        "partitioned_table" => counts.partitioned_tables += 1,
        "view" => counts.views += 1,
        "materialized_view" => counts.materialized_views += 1,
        "foreign_table" => counts.foreign_tables += 1,
        _ => counts.other += 1,
    }
}

fn diff_schema_digests(
    before: &SchemaDigestOutput,
    after: &SchemaDigestOutput,
) -> anyhow::Result<SchemaDiffOutput> {
    if before.digest_version != after.digest_version {
        anyhow::bail!(
            "schema digest versions differ: baseDigest uses v{} but current digest uses v{}",
            before.digest_version,
            after.digest_version
        );
    }

    let before_tables = before
        .tables
        .iter()
        .map(|table| (schema_table_key(table), table))
        .collect::<BTreeMap<_, _>>();
    let after_tables = after
        .tables
        .iter()
        .map(|table| (schema_table_key(table), table))
        .collect::<BTreeMap<_, _>>();

    let added_tables = after_tables
        .keys()
        .filter(|key| !before_tables.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let removed_tables = before_tables
        .keys()
        .filter(|key| !after_tables.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let mut changed_tables = Vec::new();

    for (key, before_table) in &before_tables {
        let Some(after_table) = after_tables.get(key) else {
            continue;
        };
        let diff = diff_schema_table(key, before_table, after_table);
        if diff.has_changes() {
            changed_tables.push(diff);
        }
    }

    let before_extensions = before
        .extensions
        .iter()
        .map(|extension| (extension.name.clone(), extension.version.clone()))
        .collect::<BTreeMap<_, _>>();
    let after_extensions = after
        .extensions
        .iter()
        .map(|extension| (extension.name.clone(), extension.version.clone()))
        .collect::<BTreeMap<_, _>>();
    let added_extensions = after_extensions
        .keys()
        .filter(|key| !before_extensions.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let removed_extensions = before_extensions
        .keys()
        .filter(|key| !after_extensions.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let changed_extensions = before_extensions
        .iter()
        .filter_map(|(name, before_version)| {
            after_extensions
                .get(name)
                .filter(|after_version| *after_version != before_version)
                .map(|_| name.clone())
        })
        .collect::<Vec<_>>();

    let changed = before.checksum != after.checksum
        || !added_tables.is_empty()
        || !removed_tables.is_empty()
        || !changed_tables.is_empty()
        || !added_extensions.is_empty()
        || !removed_extensions.is_empty()
        || !changed_extensions.is_empty();
    Ok(SchemaDiffOutput {
        database_id: after.database_id.clone(),
        database_name: after.database_name.clone(),
        before_checksum: before.checksum.clone(),
        after_checksum: after.checksum.clone(),
        changed,
        added_tables,
        removed_tables,
        changed_tables,
        added_extensions,
        removed_extensions,
        changed_extensions,
    })
}

impl SchemaTableDiff {
    fn has_changes(&self) -> bool {
        !self.added_columns.is_empty()
            || !self.removed_columns.is_empty()
            || !self.changed_columns.is_empty()
            || !self.added_indexes.is_empty()
            || !self.removed_indexes.is_empty()
            || !self.changed_indexes.is_empty()
            || !self.added_constraints.is_empty()
            || !self.removed_constraints.is_empty()
            || !self.changed_constraints.is_empty()
            || self.view_definition_changed
    }
}

fn diff_schema_table(
    table_key: &str,
    before: &SchemaDigestTable,
    after: &SchemaDigestTable,
) -> SchemaTableDiff {
    let before_columns = before
        .columns
        .iter()
        .map(|column| (column.name.clone(), column))
        .collect::<BTreeMap<_, _>>();
    let after_columns = after
        .columns
        .iter()
        .map(|column| (column.name.clone(), column))
        .collect::<BTreeMap<_, _>>();
    let before_indexes = before
        .indexes
        .iter()
        .map(|index| (index.name.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let after_indexes = after
        .indexes
        .iter()
        .map(|index| (index.name.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let before_constraints = before
        .constraints
        .iter()
        .map(|constraint| (constraint.name.clone(), constraint))
        .collect::<BTreeMap<_, _>>();
    let after_constraints = after
        .constraints
        .iter()
        .map(|constraint| (constraint.name.clone(), constraint))
        .collect::<BTreeMap<_, _>>();

    SchemaTableDiff {
        table: table_key.to_string(),
        added_columns: keys_added(&before_columns, &after_columns),
        removed_columns: keys_removed(&before_columns, &after_columns),
        changed_columns: before_columns
            .iter()
            .filter_map(|(name, before_column)| {
                after_columns
                    .get(name)
                    .filter(|after_column| *after_column != before_column)
                    .map(|_| name.clone())
            })
            .collect(),
        added_indexes: keys_added(&before_indexes, &after_indexes),
        removed_indexes: keys_removed(&before_indexes, &after_indexes),
        changed_indexes: before_indexes
            .iter()
            .filter_map(|(name, before_index)| {
                after_indexes
                    .get(name)
                    .filter(|after_index| *after_index != before_index)
                    .map(|_| name.clone())
            })
            .collect(),
        added_constraints: keys_added(&before_constraints, &after_constraints),
        removed_constraints: keys_removed(&before_constraints, &after_constraints),
        changed_constraints: before_constraints
            .iter()
            .filter_map(|(name, before_constraint)| {
                after_constraints
                    .get(name)
                    .filter(|after_constraint| *after_constraint != before_constraint)
                    .map(|_| name.clone())
            })
            .collect(),
        view_definition_changed: before.view_definition_hash != after.view_definition_hash,
    }
}

fn keys_added<T>(before: &BTreeMap<String, T>, after: &BTreeMap<String, T>) -> Vec<String> {
    after
        .keys()
        .filter(|key| !before.contains_key(*key))
        .cloned()
        .collect()
}

fn keys_removed<T>(before: &BTreeMap<String, T>, after: &BTreeMap<String, T>) -> Vec<String> {
    before
        .keys()
        .filter(|key| !after.contains_key(*key))
        .cloned()
        .collect()
}

fn schema_table_key(table: &SchemaDigestTable) -> String {
    format!("{}.{}", table.schema, table.name)
}

fn sha256_hex(bytes: &[u8]) -> String {
    bytes_to_hex(&Sha256::digest(bytes))
}

fn build_connection_string(
    admin_url: &str,
    database_name: &str,
    role_name: &str,
    role_password: &str,
) -> anyhow::Result<String> {
    let mut url = Url::parse(admin_url)?;
    url.set_username(role_name)
        .map_err(|_| anyhow::anyhow!("failed to set database username"))?;
    url.set_password(Some(role_password))
        .map_err(|_| anyhow::anyhow!("failed to set database password"))?;
    url.set_path(database_name);
    Ok(url.to_string())
}

#[derive(Debug, PartialEq, Eq)]
struct PgToolConnection {
    database: String,
    env: BTreeMap<String, String>,
}

async fn clone_with_pg_tools(
    source_database_url: &str,
    target_database_url: &str,
    schema_only: bool,
) -> anyhow::Result<()> {
    let source = pg_tool_connection_from_url(source_database_url)
        .context("sourceDatabaseUrl is not a supported Postgres URL")?;
    let target = pg_tool_connection_from_url(target_database_url)
        .context("target sandbox connection string is not a supported Postgres URL")?;

    let mut dump_command = Command::new("pg_dump");
    apply_command_env(&mut dump_command, &source.env);
    dump_command
        .args(pg_dump_args(&source.database, schema_only))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut dump = dump_command.spawn().context(
        "failed to start pg_dump; install PostgreSQL client tools and ensure pg_dump is on PATH",
    )?;
    let dump_stdout = dump
        .stdout
        .take()
        .context("pg_dump stdout pipe was not available")?;

    let mut restore_command = Command::new("pg_restore");
    apply_command_env(&mut restore_command, &target.env);
    restore_command
        .args(pg_restore_args(&target.database))
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut restore = restore_command.spawn().context(
        "failed to start pg_restore; install PostgreSQL client tools and ensure pg_restore is on PATH",
    )?;
    let restore_stdin = restore
        .stdin
        .take()
        .context("pg_restore stdin pipe was not available")?;

    let copy_task = tokio::spawn(async move {
        let mut dump_stdout = dump_stdout;
        let mut restore_stdin = restore_stdin;
        tokio::io::copy(&mut dump_stdout, &mut restore_stdin).await?;
        restore_stdin.shutdown().await?;
        anyhow::Ok(())
    });
    let (copy_result, dump_output_result, restore_output_result) = tokio::join!(
        copy_task,
        dump.wait_with_output(),
        restore.wait_with_output()
    );
    let dump_output = dump_output_result.context("failed to wait for pg_dump")?;
    let restore_output = restore_output_result.context("failed to wait for pg_restore")?;

    if let Some(message) = clone_tool_failure_message(
        dump_output.status.success(),
        &dump_output.stderr,
        restore_output.status.success(),
        &restore_output.stderr,
    ) {
        anyhow::bail!("{message}");
    }
    copy_result
        .context("dump/restore pipe task failed")?
        .context("dump/restore pipe failed")?;

    Ok(())
}

fn pg_tool_connection_from_url(raw_url: &str) -> anyhow::Result<PgToolConnection> {
    let parsed = Url::parse(raw_url).context("Postgres URL is invalid")?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") {
        anyhow::bail!("Postgres URL must use postgres:// or postgresql://");
    }

    let database_path = parsed.path().trim_start_matches('/');
    if database_path.is_empty() || database_path.contains('/') {
        anyhow::bail!("Postgres URL must include a single database name path segment");
    }

    let mut env = BTreeMap::new();
    if let Some(host) = parsed.host_str() {
        env.insert("PGHOST".to_string(), host.to_string());
    }
    if let Some(port) = parsed.port() {
        env.insert("PGPORT".to_string(), port.to_string());
    }
    if !parsed.username().is_empty() {
        env.insert(
            "PGUSER".to_string(),
            percent_decode_url_component(parsed.username())
                .context("Postgres username is invalid")?,
        );
    }
    if let Some(password) = parsed.password() {
        env.insert(
            "PGPASSWORD".to_string(),
            percent_decode_url_component(password).context("Postgres password is invalid")?,
        );
    }
    for (key, value) in parsed.query_pairs() {
        let pg_env_key = if key.eq_ignore_ascii_case("sslmode") {
            Some("PGSSLMODE")
        } else if key.eq_ignore_ascii_case("sslrootcert") {
            Some("PGSSLROOTCERT")
        } else if key.eq_ignore_ascii_case("sslcert") {
            Some("PGSSLCERT")
        } else if key.eq_ignore_ascii_case("sslkey") {
            Some("PGSSLKEY")
        } else {
            None
        };
        if let Some(env_key) = pg_env_key {
            env.insert(env_key.to_string(), value.into_owned());
        }
    }

    Ok(PgToolConnection {
        database: percent_decode_url_component(database_path)
            .context("Postgres database name is invalid")?,
        env,
    })
}

fn pg_dump_args(database: &str, schema_only: bool) -> Vec<String> {
    let mut args = vec![
        "--format=custom".to_string(),
        "--no-owner".to_string(),
        "--no-privileges".to_string(),
    ];
    if schema_only {
        args.push("--schema-only".to_string());
    }
    args.extend(["--dbname".to_string(), database.to_string()]);
    args
}

fn pg_restore_args(database: &str) -> Vec<String> {
    vec![
        "--no-owner".to_string(),
        "--no-privileges".to_string(),
        "--exit-on-error".to_string(),
        "--single-transaction".to_string(),
        "--dbname".to_string(),
        database.to_string(),
    ]
}

fn percent_decode_url_component(value: &str) -> anyhow::Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                anyhow::bail!("incomplete percent escape");
            }
            let high = hex_value(bytes[index + 1]).context("invalid percent escape")?;
            let low = hex_value(bytes[index + 2]).context("invalid percent escape")?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).context("decoded URL component is not valid UTF-8")
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn summarize_tool_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    if text.is_empty() {
        return "(no stderr)".to_string();
    }
    const MAX_ERROR_LEN: usize = 4_000;
    if text.len() > MAX_ERROR_LEN {
        let mut end = MAX_ERROR_LEN;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &text[..end])
    } else {
        text
    }
}

fn clone_tool_failure_message(
    dump_success: bool,
    dump_stderr: &[u8],
    restore_success: bool,
    restore_stderr: &[u8],
) -> Option<String> {
    if !restore_success {
        return Some(format!(
            "pg_restore failed: {}",
            summarize_tool_stderr(restore_stderr)
        ));
    }
    if !dump_success {
        return Some(format!(
            "pg_dump failed: {}",
            summarize_tool_stderr(dump_stderr)
        ));
    }
    None
}

fn clamp_ttl(ttl_minutes: Option<u32>, profile: &SandboxProfile) -> anyhow::Result<u32> {
    let ttl_minutes = ttl_minutes.unwrap_or(profile.default_ttl_minutes);
    if ttl_minutes > profile.max_ttl_minutes {
        anyhow::bail!(
            "ttlMinutes exceeds maxTtlMinutes ({}) for profile {}",
            profile.max_ttl_minutes,
            profile.name
        );
    }
    Ok(ttl_minutes)
}

pub fn assert_safe_readonly_sql(sql: &str) -> anyhow::Result<()> {
    let tokens = sql_keyword_tokens(sql);
    for (index, token) in tokens.iter().enumerate() {
        if matches!(
            token.as_str(),
            "begin" | "commit" | "rollback" | "abort" | "end" | "savepoint" | "release" | "reset"
        ) || (token == "set"
            && tokens
                .get(index + 1)
                .is_some_and(|next| matches!(next.as_str(), "session" | "transaction" | "local")))
        {
            anyhow::bail!(
                "readonly SQL cannot include transaction-control or session-setting statements."
            );
        }
    }
    Ok(())
}

fn explainable_statement(sql: &str) -> anyhow::Result<&str> {
    assert_safe_readonly_sql(sql)?;
    let statement = single_sql_statement(sql)?;
    let keyword = first_sql_keyword(statement).context("explainQuery sql cannot be empty")?;
    if !matches!(
        keyword.as_str(),
        "select" | "with" | "values" | "table" | "insert" | "update" | "delete" | "merge"
    ) {
        anyhow::bail!(
            "explainQuery only accepts SELECT, WITH, VALUES, TABLE, INSERT, UPDATE, DELETE, or MERGE statements."
        );
    }
    Ok(statement)
}

fn single_sql_statement(sql: &str) -> anyhow::Result<&str> {
    let bytes = sql.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if skip_sql_noise(bytes, &mut index) {
            continue;
        }
        if bytes[index] == b';' {
            let statement = sql[..index].trim();
            index += 1;
            while index < bytes.len() {
                if skip_sql_noise(bytes, &mut index) {
                    continue;
                }
                anyhow::bail!("explainQuery only accepts a single SQL statement.");
            }
            if statement.is_empty() {
                anyhow::bail!("explainQuery sql cannot be empty");
            }
            return Ok(statement);
        }
        index += 1;
    }

    let statement = sql.trim();
    if statement.is_empty() {
        anyhow::bail!("explainQuery sql cannot be empty");
    }
    Ok(statement)
}

fn explain_summary(plan: &Value) -> Value {
    let root_plan = plan
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.get("Plan"));
    let Some(root_plan) = root_plan else {
        return json!({ "nodeCount": 0 });
    };

    let mut node_types = BTreeSet::new();
    let mut relations = BTreeSet::new();
    let mut node_count = 0usize;
    collect_explain_summary(root_plan, &mut node_types, &mut relations, &mut node_count);

    json!({
        "topNode": root_plan.get("Node Type").and_then(Value::as_str),
        "totalCost": root_plan.get("Total Cost").and_then(Value::as_f64),
        "planRows": root_plan.get("Plan Rows").and_then(Value::as_i64),
        "nodeCount": node_count,
        "nodeTypes": node_types.into_iter().collect::<Vec<_>>(),
        "relations": relations.into_iter().collect::<Vec<_>>()
    })
}

fn collect_explain_summary(
    plan: &Value,
    node_types: &mut BTreeSet<String>,
    relations: &mut BTreeSet<String>,
    node_count: &mut usize,
) {
    *node_count += 1;
    if let Some(node_type) = plan.get("Node Type").and_then(Value::as_str) {
        node_types.insert(node_type.to_string());
    }
    if let Some(relation) = plan.get("Relation Name").and_then(Value::as_str) {
        relations.insert(relation.to_string());
    }
    if let Some(plans) = plan.get("Plans").and_then(Value::as_array) {
        for child in plans {
            collect_explain_summary(child, node_types, relations, node_count);
        }
    }
}

async fn run_readonly_query(
    client: &Client,
    sql: &str,
    row_limit: usize,
) -> anyhow::Result<QueryExecutionResult> {
    assert_safe_readonly_sql(sql)?;
    client.batch_execute("BEGIN READ ONLY").await?;

    let result = run_sql_body(client, sql, row_limit, false).await;
    let rollback = client.batch_execute("ROLLBACK").await;

    match (result, rollback) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), _) => {
            if let Some(message) = readonly_violation_message(sql, &error) {
                anyhow::bail!("{message}");
            }
            Err(error)
        }
        (Ok(_), Err(error)) => Err(error.into()),
    }
}

async fn run_sql_body(
    client: &Client,
    sql: &str,
    row_limit: usize,
    cursor_owns_transaction: bool,
) -> anyhow::Result<QueryExecutionResult> {
    match query_mode(sql) {
        QueryMode::Cursor => {
            run_cursor_query(client, sql, row_limit, cursor_owns_transaction).await
        }
        QueryMode::TypedRows => run_typed_query(client, sql, row_limit).await,
        QueryMode::Simple => run_direct_query(client, sql, row_limit).await,
    }
}

async fn run_cursor_query(
    client: &Client,
    sql: &str,
    row_limit: usize,
    owns_transaction: bool,
) -> anyhow::Result<QueryExecutionResult> {
    let cursor_name = format!("pgsandbox_cursor_{}", Uuid::new_v4().simple());
    let quoted_cursor = quote_ident(&cursor_name)?;
    if owns_transaction {
        client.batch_execute("BEGIN").await?;
    }
    let declare_result = client
        .batch_execute(&format!(
            "DECLARE {quoted_cursor} NO SCROLL CURSOR FOR {sql}"
        ))
        .await;
    if let Err(error) = declare_result {
        if owns_transaction {
            let _ = client.batch_execute("ROLLBACK").await;
        }
        return Err(error.into());
    }

    let rows_result = client
        .query(
            &format!("FETCH FORWARD {} FROM {quoted_cursor}", row_limit + 1),
            &[],
        )
        .await;
    let rows = match rows_result {
        Ok(rows) => rows,
        Err(error) => {
            let _ = client
                .batch_execute(&format!("CLOSE {quoted_cursor}"))
                .await;
            if owns_transaction {
                let _ = client.batch_execute("ROLLBACK").await;
            }
            return Err(error.into());
        }
    };
    let _ = client
        .batch_execute(&format!("CLOSE {quoted_cursor}"))
        .await;
    if owns_transaction {
        client.batch_execute("COMMIT").await?;
    }

    let truncated = rows.len() > row_limit;
    let visible_rows = rows.into_iter().take(row_limit).collect::<Vec<_>>();
    let returned_row_count = visible_rows.len();
    Ok(QueryExecutionResult {
        row_count: if truncated {
            None
        } else {
            Some(returned_row_count as u64)
        },
        returned_row_count,
        affected_row_count: None,
        total_row_count_known: !truncated,
        rows: rows_to_json(visible_rows)?,
        truncated,
    })
}

async fn run_typed_query(
    client: &Client,
    sql: &str,
    row_limit: usize,
) -> anyhow::Result<QueryExecutionResult> {
    let limited_sql = dml_returning_limit_sql(sql, row_limit);
    let rows = client
        .query(limited_sql.as_deref().unwrap_or(sql), &[])
        .await?;
    let truncated = rows.len() > row_limit;
    let visible_rows = rows.into_iter().take(row_limit).collect::<Vec<_>>();
    let returned_row_count = visible_rows.len();
    Ok(QueryExecutionResult {
        row_count: if truncated {
            None
        } else {
            Some(returned_row_count as u64)
        },
        returned_row_count,
        affected_row_count: None,
        total_row_count_known: !truncated,
        rows: rows_to_json(visible_rows)?,
        truncated,
    })
}

fn dml_returning_limit_sql(sql: &str, row_limit: usize) -> Option<String> {
    let first_keyword = first_sql_keyword(sql)?;
    if !matches!(
        first_keyword.as_str(),
        "insert" | "update" | "delete" | "merge"
    ) || !contains_sql_keyword(sql, "returning")
    {
        return None;
    }
    let trimmed = sql.trim().trim_end_matches(';').trim_end();
    let alias = returning_limit_alias(trimmed);
    Some(format!(
        "WITH {alias} AS (\n{trimmed}\n) SELECT * FROM {alias} LIMIT {}",
        row_limit + 1
    ))
}

fn readonly_violation_message(sql: &str, error: &anyhow::Error) -> Option<String> {
    if !is_readonly_violation_error(error) {
        return None;
    }
    let statement = first_sql_keyword(sql)
        .map(|keyword| keyword.to_ascii_uppercase())
        .unwrap_or_else(|| "SQL".to_string());
    Some(format!(
        "readonly=true blocked {statement} statement; readonly=true runs SQL in a read-only transaction. Database detail: {error:#}"
    ))
}

fn is_readonly_violation_error(error: &anyhow::Error) -> bool {
    if let Some(pg_error) = error.downcast_ref::<tokio_postgres::Error>() {
        if pg_error
            .as_db_error()
            .is_some_and(|db_error| db_error.code() == &SqlState::READ_ONLY_SQL_TRANSACTION)
        {
            return true;
        }
    }
    error.chain().any(|cause| {
        cause
            .to_string()
            .to_ascii_lowercase()
            .contains("read-only transaction")
    })
}

fn returning_limit_alias(sql: &str) -> String {
    let digest = Sha256::digest(sql.as_bytes());
    let mut suffix = String::with_capacity(16);
    for byte in &digest[..8] {
        suffix.push_str(&format!("{byte:02x}"));
    }
    format!("pgsandbox_limited_returning_{suffix}")
}

async fn run_direct_query(
    client: &Client,
    sql: &str,
    row_limit: usize,
) -> anyhow::Result<QueryExecutionResult> {
    let messages = client.simple_query(sql).await?;
    Ok(format_simple_query_result(messages, row_limit))
}

fn format_simple_query_result(
    messages: Vec<SimpleQueryMessage>,
    row_limit: usize,
) -> QueryExecutionResult {
    let mut current_rows = Vec::new();
    let mut current_had_rows = false;
    let mut final_rows = Vec::new();
    let mut final_had_rows = false;
    let mut final_row_count = None;

    for message in messages {
        match message {
            SimpleQueryMessage::Row(row) => {
                if !current_had_rows {
                    current_rows.clear();
                    current_had_rows = true;
                }
                let mut object = serde_json::Map::new();
                for (index, column) in row.columns().iter().enumerate() {
                    object.insert(
                        column.name().to_string(),
                        row.get(index)
                            .map_or(Value::Null, |value| Value::String(value.to_string())),
                    );
                }
                current_rows.push(Value::Object(object));
            }
            SimpleQueryMessage::CommandComplete(count) => {
                final_row_count = Some(count);
                final_had_rows = current_had_rows;
                final_rows = if current_had_rows {
                    std::mem::take(&mut current_rows)
                } else {
                    Vec::new()
                };
                current_had_rows = false;
            }
            _ => {}
        }
    }

    let truncated = final_rows.len() > row_limit;
    let visible_rows = final_rows.into_iter().take(row_limit).collect::<Vec<_>>();
    let returned_row_count = if final_had_rows {
        visible_rows.len()
    } else {
        0
    };
    QueryExecutionResult {
        row_count: if final_had_rows && truncated {
            None
        } else {
            final_row_count
        },
        returned_row_count,
        affected_row_count: if final_had_rows {
            None
        } else {
            final_row_count
        },
        total_row_count_known: !final_had_rows || !truncated,
        rows: visible_rows,
        truncated,
    }
}

fn query_mode(sql: &str) -> QueryMode {
    if CURSOR_QUERY_RE.is_match(sql) {
        return QueryMode::Cursor;
    }
    if TYPED_ROW_PREFIX_RE.is_match(sql) || contains_sql_keyword(sql, "returning") {
        return QueryMode::TypedRows;
    }
    QueryMode::Simple
}

fn first_sql_keyword(sql: &str) -> Option<String> {
    sql_keyword_tokens(sql).into_iter().next()
}

fn contains_sql_keyword(sql: &str, keyword: &str) -> bool {
    sql_keyword_tokens(sql)
        .iter()
        .any(|token| token.eq_ignore_ascii_case(keyword))
}

fn sql_keyword_tokens(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut index = 0;
    let mut tokens = Vec::new();
    while index < bytes.len() {
        if skip_sql_noise(bytes, &mut index) {
            continue;
        }
        if is_ident_start(bytes[index]) {
            let start = index;
            index += 1;
            while index < bytes.len() && is_ident_part(bytes[index]) {
                index += 1;
            }
            if let Ok(token) = std::str::from_utf8(&bytes[start..index]) {
                tokens.push(token.to_ascii_lowercase());
            }
            continue;
        }
        index += 1;
    }
    tokens
}

fn skip_sql_noise(bytes: &[u8], index: &mut usize) -> bool {
    if bytes[*index].is_ascii_whitespace() {
        *index += 1;
        return true;
    }
    if bytes[*index..].starts_with(b"--") {
        *index += 2;
        while *index < bytes.len() && bytes[*index] != b'\n' {
            *index += 1;
        }
        return true;
    }
    if bytes[*index..].starts_with(b"/*") {
        *index += 2;
        while *index + 1 < bytes.len() && !bytes[*index..].starts_with(b"*/") {
            *index += 1;
        }
        *index = (*index + 2).min(bytes.len());
        return true;
    }
    if bytes[*index] == b'\'' {
        skip_quoted(bytes, index, b'\'');
        return true;
    }
    if bytes[*index] == b'"' {
        skip_quoted(bytes, index, b'"');
        return true;
    }
    if bytes[*index] == b'$' && skip_dollar_quoted(bytes, index) {
        return true;
    }
    false
}

fn skip_quoted(bytes: &[u8], index: &mut usize, quote: u8) {
    *index += 1;
    while *index < bytes.len() {
        if bytes[*index] == quote {
            if *index + 1 < bytes.len() && bytes[*index + 1] == quote {
                *index += 2;
            } else {
                *index += 1;
                break;
            }
        } else {
            *index += 1;
        }
    }
}

fn skip_dollar_quoted(bytes: &[u8], index: &mut usize) -> bool {
    let start = *index;
    let mut end = start + 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    if end >= bytes.len() || bytes[end] != b'$' {
        return false;
    }

    let delimiter = &bytes[start..=end];
    *index = end + 1;
    while *index + delimiter.len() <= bytes.len() {
        if bytes[*index..].starts_with(delimiter) {
            *index += delimiter.len();
            return true;
        }
        *index += 1;
    }
    *index = bytes.len();
    true
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_part(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn protect_role_password(password: &str, profile: &SandboxProfile) -> anyhow::Result<String> {
    let cipher = role_password_cipher(profile)?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let encrypted = cipher
        .encrypt(&nonce, password.as_bytes())
        .map_err(|_| anyhow::anyhow!("failed to encrypt sandbox role password"))?;

    Ok(format!(
        "{ENCRYPTED_PASSWORD_PREFIX}:{}:{}",
        URL_SAFE_NO_PAD.encode(nonce),
        URL_SAFE_NO_PAD.encode(encrypted)
    ))
}

fn unprotect_role_password(value: &str, profile: &SandboxProfile) -> anyhow::Result<String> {
    let Some(rest) = value.strip_prefix(&format!("{ENCRYPTED_PASSWORD_PREFIX}:")) else {
        return Ok(value.to_string());
    };
    let Some((nonce, encrypted)) = rest.split_once(':') else {
        anyhow::bail!("stored sandbox role password is malformed");
    };
    let nonce = URL_SAFE_NO_PAD
        .decode(nonce)
        .context("stored sandbox role password nonce is invalid")?;
    let encrypted = URL_SAFE_NO_PAD
        .decode(encrypted)
        .context("stored sandbox role password ciphertext is invalid")?;

    if nonce.len() != 12 {
        anyhow::bail!("stored sandbox role password nonce has invalid length");
    }

    let cipher = role_password_cipher(profile)?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce), encrypted.as_ref())
        .map_err(|_| {
            anyhow::anyhow!(
                "failed to decrypt sandbox role password; the admin URL may have changed"
            )
        })?;

    String::from_utf8(plaintext).context("stored sandbox role password is not valid UTF-8")
}

fn role_password_cipher(profile: &SandboxProfile) -> anyhow::Result<Aes256Gcm> {
    let mut hasher = Sha256::new();
    hasher.update(b"pgsandbox-mcp sandbox role password v1\0");
    hasher.update(profile.admin_url.as_bytes());
    let key = hasher.finalize();
    Aes256Gcm::new_from_slice(&key)
        .map_err(|_| anyhow::anyhow!("failed to initialize role password cipher"))
}

fn sandbox_record_from_row(row: &Row) -> SandboxRecord {
    SandboxRecord {
        database_id: row.get("database_id"),
        profile_name: row.get("profile_name"),
        database_name: row.get("database_name"),
        role_name: row.get("role_name"),
        role_password: row.get("role_password"),
        owner: row.get("owner"),
        purpose: row.get("purpose"),
        labels: row.get("labels"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        deleted_at: row.get("deleted_at"),
    }
}

fn record_summary_to_json(row: &Row) -> Value {
    let database_id = row.get::<_, String>("database_id");
    let profile_name = row.get::<_, String>("profile_name");
    let database_name = row.get::<_, String>("database_name");
    let role_name = row.get::<_, String>("role_name");
    let owner = row.get::<_, Option<String>>("owner");
    let purpose = row.get::<_, Option<String>>("purpose");
    let labels = row.get::<_, Value>("labels");
    let created_at = row.get::<_, DateTime<Utc>>("created_at");
    let expires_at = row.get::<_, DateTime<Utc>>("expires_at");
    let deleted_at = row.get::<_, Option<DateTime<Utc>>>("deleted_at");
    json!({
        "databaseId": database_id,
        "profile": profile_name,
        "databaseName": database_name,
        "roleName": role_name,
        "owner": owner,
        "purpose": purpose,
        "labels": labels,
        "createdAt": created_at,
        "expiresAt": expires_at,
        "deletedAt": deleted_at,
        "database_id": database_id,
        "profile_name": profile_name,
        "database_name": database_name,
        "role_name": role_name,
        "created_at": created_at,
        "expires_at": expires_at,
        "deleted_at": deleted_at,
    })
}

fn record_to_json(record: &SandboxRecord) -> Value {
    json!({
        "database_id": record.database_id,
        "profile_name": record.profile_name,
        "database_name": record.database_name,
        "role_name": record.role_name,
        "owner": record.owner,
        "purpose": record.purpose,
        "labels": record.labels,
        "created_at": record.created_at,
        "expires_at": record.expires_at,
        "deleted_at": record.deleted_at,
    })
}

fn rows_to_json(rows: Vec<Row>) -> anyhow::Result<Vec<Value>> {
    rows.iter().map(row_to_json).collect()
}

fn row_to_json(row: &Row) -> anyhow::Result<Value> {
    let mut object = serde_json::Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        object.insert(
            column.name().to_string(),
            cell_to_json(row, index, column.type_())
                .with_context(|| format!("failed to serialize column {}", column.name()))?,
        );
    }
    Ok(Value::Object(object))
}

fn cell_to_json(row: &Row, index: usize, value_type: &Type) -> anyhow::Result<Value> {
    if matches!(
        value_type,
        &Type::TEXT | &Type::VARCHAR | &Type::BPCHAR | &Type::NAME
    ) {
        return Ok(row
            .try_get::<_, Option<String>>(index)
            .ok()
            .flatten()
            .map(Value::String)
            .unwrap_or(Value::Null));
    }

    if let Some(kind) = array_cell_kind(value_type) {
        return Ok(array_cell_to_json(row, index, kind));
    }

    let value = match *value_type {
        Type::BOOL => row
            .try_get::<_, Option<bool>>(index)
            .ok()
            .flatten()
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        Type::INT2 => row
            .try_get::<_, Option<i16>>(index)
            .ok()
            .flatten()
            .map(|value| json!(value))
            .unwrap_or(Value::Null),
        Type::INT4 => row
            .try_get::<_, Option<i32>>(index)
            .ok()
            .flatten()
            .map(|value| json!(value))
            .unwrap_or(Value::Null),
        Type::INT8 => row
            .try_get::<_, Option<i64>>(index)
            .ok()
            .flatten()
            .map(|value| json!(value))
            .unwrap_or(Value::Null),
        Type::FLOAT4 => row
            .try_get::<_, Option<f32>>(index)
            .ok()
            .flatten()
            .map(|value| json!(value))
            .unwrap_or(Value::Null),
        Type::FLOAT8 => row
            .try_get::<_, Option<f64>>(index)
            .ok()
            .flatten()
            .map(|value| json!(value))
            .unwrap_or(Value::Null),
        Type::NUMERIC => row
            .try_get::<_, Option<PgNumeric>>(index)
            .ok()
            .flatten()
            .map(|value| Value::String(value.0))
            .unwrap_or(Value::Null),
        Type::BYTEA => row
            .try_get::<_, Option<Vec<u8>>>(index)
            .ok()
            .flatten()
            .map(|value| Value::String(format!("\\x{}", bytes_to_hex(&value))))
            .unwrap_or(Value::Null),
        Type::OID => row
            .try_get::<_, Option<u32>>(index)
            .ok()
            .flatten()
            .map(|value| json!(value))
            .unwrap_or(Value::Null),
        Type::JSON | Type::JSONB => row
            .try_get::<_, Option<Value>>(index)
            .ok()
            .flatten()
            .unwrap_or(Value::Null),
        Type::TIMESTAMPTZ => row
            .try_get::<_, Option<DateTime<Utc>>>(index)
            .ok()
            .flatten()
            .map(|value| Value::String(value.to_rfc3339()))
            .unwrap_or(Value::Null),
        Type::TIMESTAMP => row
            .try_get::<_, Option<NaiveDateTime>>(index)
            .ok()
            .flatten()
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
        Type::DATE => row
            .try_get::<_, Option<NaiveDate>>(index)
            .ok()
            .flatten()
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
        Type::TIME => row
            .try_get::<_, Option<NaiveTime>>(index)
            .ok()
            .flatten()
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
        Type::TIMETZ => row
            .try_get::<_, Option<PgTimeTz>>(index)
            .ok()
            .flatten()
            .map(|value| Value::String(value.0))
            .unwrap_or(Value::Null),
        Type::UUID => row
            .try_get::<_, Option<Uuid>>(index)
            .ok()
            .flatten()
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
        _ => {
            let type_name = value_type.name();
            Value::String(format!("<unsupported Postgres type {type_name}>"))
        }
    };
    Ok(value)
}

fn array_cell_kind(value_type: &Type) -> Option<ArrayCellKind> {
    match *value_type {
        Type::TEXT_ARRAY | Type::VARCHAR_ARRAY | Type::BPCHAR_ARRAY | Type::NAME_ARRAY => {
            Some(ArrayCellKind::Text)
        }
        Type::BOOL_ARRAY => Some(ArrayCellKind::Bool),
        Type::INT2_ARRAY => Some(ArrayCellKind::Int2),
        Type::INT4_ARRAY => Some(ArrayCellKind::Int4),
        Type::INT8_ARRAY => Some(ArrayCellKind::Int8),
        Type::FLOAT4_ARRAY => Some(ArrayCellKind::Float4),
        Type::FLOAT8_ARRAY => Some(ArrayCellKind::Float8),
        Type::JSON_ARRAY | Type::JSONB_ARRAY => Some(ArrayCellKind::Json),
        Type::DATE_ARRAY => Some(ArrayCellKind::Date),
        Type::TIMESTAMP_ARRAY => Some(ArrayCellKind::Timestamp),
        Type::TIMESTAMPTZ_ARRAY => Some(ArrayCellKind::TimestampTz),
        Type::UUID_ARRAY => Some(ArrayCellKind::Uuid),
        _ => None,
    }
}

fn array_cell_to_json(row: &Row, index: usize, kind: ArrayCellKind) -> Value {
    match kind {
        ArrayCellKind::Text => row
            .try_get::<_, Option<Vec<Option<String>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, Value::String))
            .unwrap_or(Value::Null),
        ArrayCellKind::Bool => row
            .try_get::<_, Option<Vec<Option<bool>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, Value::Bool))
            .unwrap_or(Value::Null),
        ArrayCellKind::Int2 => row
            .try_get::<_, Option<Vec<Option<i16>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, |value| json!(value)))
            .unwrap_or(Value::Null),
        ArrayCellKind::Int4 => row
            .try_get::<_, Option<Vec<Option<i32>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, |value| json!(value)))
            .unwrap_or(Value::Null),
        ArrayCellKind::Int8 => row
            .try_get::<_, Option<Vec<Option<i64>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, |value| json!(value)))
            .unwrap_or(Value::Null),
        ArrayCellKind::Float4 => row
            .try_get::<_, Option<Vec<Option<f32>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, |value| json!(value)))
            .unwrap_or(Value::Null),
        ArrayCellKind::Float8 => row
            .try_get::<_, Option<Vec<Option<f64>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, |value| json!(value)))
            .unwrap_or(Value::Null),
        ArrayCellKind::Json => row
            .try_get::<_, Option<Vec<Option<Value>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, |value| value))
            .unwrap_or(Value::Null),
        ArrayCellKind::Date => row
            .try_get::<_, Option<Vec<Option<NaiveDate>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, |value| Value::String(value.to_string())))
            .unwrap_or(Value::Null),
        ArrayCellKind::Timestamp => row
            .try_get::<_, Option<Vec<Option<NaiveDateTime>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, |value| Value::String(value.to_string())))
            .unwrap_or(Value::Null),
        ArrayCellKind::TimestampTz => row
            .try_get::<_, Option<Vec<Option<DateTime<Utc>>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, |value| Value::String(value.to_rfc3339())))
            .unwrap_or(Value::Null),
        ArrayCellKind::Uuid => row
            .try_get::<_, Option<Vec<Option<Uuid>>>>(index)
            .ok()
            .map(|value| optional_array_to_json(value, |value| Value::String(value.to_string())))
            .unwrap_or(Value::Null),
    }
}

fn optional_array_to_json<T, F>(value: Option<Vec<Option<T>>>, mapper: F) -> Value
where
    F: Fn(T) -> Value,
{
    match value {
        Some(items) => Value::Array(
            items
                .into_iter()
                .map(|item| item.map(&mapper).unwrap_or(Value::Null))
                .collect(),
        ),
        None => Value::Null,
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn decode_pg_numeric(raw: &[u8]) -> anyhow::Result<String> {
    let mut offset = 0;
    let ndigits = read_i16(raw, &mut offset)? as usize;
    let weight = read_i16(raw, &mut offset)?;
    let sign = read_u16(raw, &mut offset)?;
    let dscale = read_i16(raw, &mut offset)?;
    if dscale < 0 {
        anyhow::bail!("invalid NUMERIC scale");
    }

    let mut digits = Vec::with_capacity(ndigits);
    for _ in 0..ndigits {
        let digit = read_i16(raw, &mut offset)?;
        if !(0..=9999).contains(&digit) {
            anyhow::bail!("invalid NUMERIC digit");
        }
        digits.push(digit as u16);
    }

    if offset != raw.len() {
        anyhow::bail!("invalid NUMERIC payload length");
    }

    match sign {
        0xC000 => return Ok("NaN".to_string()),
        0xD000 => return Ok("Infinity".to_string()),
        0xF000 => return Ok("-Infinity".to_string()),
        0x0000 | 0x4000 => {}
        _ => anyhow::bail!("invalid NUMERIC sign"),
    }

    let integer_groups = i32::from(weight) + 1;
    let mut integer = String::new();
    if integer_groups > 0 {
        for group_index in 0..integer_groups as usize {
            let digit = digits.get(group_index).copied().unwrap_or(0);
            if group_index == 0 {
                integer.push_str(&digit.to_string());
            } else {
                integer.push_str(&format!("{digit:04}"));
            }
        }
    }
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };

    let scale = dscale as usize;
    let mut fraction = String::new();
    let fractional_start = integer_groups.max(0) as usize;
    if integer_groups < 0 {
        for _ in 0..(-integer_groups) {
            fraction.push_str("0000");
        }
    }
    for digit in digits.iter().skip(fractional_start) {
        fraction.push_str(&format!("{digit:04}"));
    }
    if fraction.len() < scale {
        fraction.push_str(&"0".repeat(scale - fraction.len()));
    }
    fraction.truncate(scale);

    let mut value = String::new();
    if sign == 0x4000 && (integer != "0" || fraction.chars().any(|c| c != '0')) {
        value.push('-');
    }
    value.push_str(integer);
    if scale > 0 {
        value.push('.');
        value.push_str(&fraction);
    }
    Ok(value)
}

fn decode_pg_timetz(raw: &[u8]) -> anyhow::Result<String> {
    let mut offset = 0;
    let micros = read_i64(raw, &mut offset)?;
    let timezone_seconds_west = read_i32(raw, &mut offset)?;
    if offset != raw.len() {
        anyhow::bail!("invalid TIMETZ payload length");
    }
    if !(0..86_400_000_000).contains(&micros) {
        anyhow::bail!("invalid TIMETZ time");
    }

    let seconds = (micros / 1_000_000) as u32;
    let nanos = ((micros % 1_000_000) * 1_000) as u32;
    let time = NaiveTime::from_num_seconds_from_midnight_opt(seconds, nanos)
        .context("invalid TIMETZ time")?;
    let offset_seconds_east = -timezone_seconds_west;
    let sign = if offset_seconds_east >= 0 { '+' } else { '-' };
    let absolute_offset = offset_seconds_east.abs();
    let hours = absolute_offset / 3600;
    let minutes = (absolute_offset % 3600) / 60;
    Ok(format!("{time}{sign}{hours:02}:{minutes:02}"))
}

fn read_i16(raw: &[u8], offset: &mut usize) -> anyhow::Result<i16> {
    if raw.len() < *offset + 2 {
        anyhow::bail!("invalid NUMERIC payload length");
    }
    let value = i16::from_be_bytes([raw[*offset], raw[*offset + 1]]);
    *offset += 2;
    Ok(value)
}

fn read_u16(raw: &[u8], offset: &mut usize) -> anyhow::Result<u16> {
    if raw.len() < *offset + 2 {
        anyhow::bail!("invalid NUMERIC payload length");
    }
    let value = u16::from_be_bytes([raw[*offset], raw[*offset + 1]]);
    *offset += 2;
    Ok(value)
}

fn read_i32(raw: &[u8], offset: &mut usize) -> anyhow::Result<i32> {
    if raw.len() < *offset + 4 {
        anyhow::bail!("invalid payload length");
    }
    let value = i32::from_be_bytes([
        raw[*offset],
        raw[*offset + 1],
        raw[*offset + 2],
        raw[*offset + 3],
    ]);
    *offset += 4;
    Ok(value)
}

fn read_i64(raw: &[u8], offset: &mut usize) -> anyhow::Result<i64> {
    if raw.len() < *offset + 8 {
        anyhow::bail!("invalid payload length");
    }
    let value = i64::from_be_bytes([
        raw[*offset],
        raw[*offset + 1],
        raw[*offset + 2],
        raw[*offset + 3],
        raw[*offset + 4],
        raw[*offset + 5],
        raw[*offset + 6],
        raw[*offset + 7],
    ]);
    *offset += 8;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SandboxConfig {
        SandboxConfig {
            default_profile: "default".to_string(),
            profiles: vec![SandboxProfile {
                name: "default".to_string(),
                admin_url: "postgres://postgres:secret@localhost:5432/postgres?sslmode=disable"
                    .to_string(),
                database_prefix: "pgsandbox".to_string(),
                default_ttl_minutes: 240,
                max_ttl_minutes: 1440,
                allow_external_admin_url: false,
                allowed_admin_hosts: Vec::new(),
                max_active_databases_per_owner: None,
                postgres_version: None,
                managed_local: false,
            }],
            telemetry: crate::config::TelemetryConfig { enabled: false },
            managed_local: crate::config::ManagedLocalConfig { enabled: false },
        }
    }

    fn test_database_url() -> &'static str {
        "postgres://sandbox:secret@127.0.0.1:5432/app?sslmode=disable"
    }

    #[tokio::test]
    async fn execute_repo_command_does_not_inherit_mcp_stdin() {
        let current_exe = std::env::current_exe().unwrap();
        let mut child = Command::new(current_exe)
            .arg("execute_repo_command_inherited_stdin_helper")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env("PGSANDBOX_RUN_STDIN_HELPER", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let _stdin_guard = child.stdin.take().expect("helper stdin pipe");
        let stdout = child.stdout.take().expect("helper stdout pipe");
        let stderr = child.stderr.take().expect("helper stderr pipe");
        let stdout_task = tokio::spawn(read_bounded_output(stdout));
        let stderr_task = tokio::spawn(read_bounded_output(stderr));
        let status = time::timeout(StdDuration::from_secs(5), child.wait())
            .await
            .expect("helper test did not finish")
            .expect("helper test process failed");
        let (stdout, _) = stdout_task.await.unwrap().unwrap();
        let (stderr, _) = stderr_task.await.unwrap().unwrap();

        assert!(
            status.success(),
            "helper test failed with {status}; stdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    #[tokio::test]
    async fn execute_repo_command_inherited_stdin_helper() {
        if std::env::var("PGSANDBOX_RUN_STDIN_HELPER").ok().as_deref() != Some("1") {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "if IFS= read -r _; then echo inherited-stdin; exit 12; else echo stdin-closed; fi"
                .to_string(),
        ];

        let result = execute_repo_command(
            directory.path(),
            &command,
            test_database_url(),
            StdDuration::from_secs(1),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, Some(0), "{result:?}");
        assert_eq!(result.stdout.trim(), "stdin-closed");
        assert!(!result.stderr.contains("timed out"), "{result:?}");
    }

    #[tokio::test]
    async fn execute_repo_command_captures_success_output() {
        let directory = tempfile::tempdir().unwrap();
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf success; printf warning >&2".to_string(),
        ];

        let result = execute_repo_command(
            directory.path(),
            &command,
            test_database_url(),
            StdDuration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "success");
        assert_eq!(result.stderr, "warning");
        assert!(!result.stdout_truncated);
        assert!(!result.stderr_truncated);
    }

    #[tokio::test]
    async fn execute_repo_command_captures_failure_output() {
        let directory = tempfile::tempdir().unwrap();
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf failed >&2; exit 17".to_string(),
        ];

        let result = execute_repo_command(
            directory.path(),
            &command,
            test_database_url(),
            StdDuration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, Some(17));
        assert_eq!(result.stdout, "");
        assert_eq!(result.stderr, "failed");
    }

    #[tokio::test]
    async fn execute_repo_command_honors_timeout() {
        let directory = tempfile::tempdir().unwrap();
        let command = vec!["sh".to_string(), "-c".to_string(), "sleep 5".to_string()];

        let result = execute_repo_command(
            directory.path(),
            &command,
            test_database_url(),
            StdDuration::from_secs(1),
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, None);
        assert!(result.elapsed_ms < 3_000, "{result:?}");
        assert!(result.stderr.contains("timed out after 1 seconds"));
    }

    #[test]
    fn readonly_guard_rejects_transaction_control() {
        assert!(assert_safe_readonly_sql("select * from users").is_ok());
        assert!(assert_safe_readonly_sql("select 'rollback' as stage").is_ok());
        assert!(assert_safe_readonly_sql("select $$commit$$ as stage").is_ok());
        assert!(assert_safe_readonly_sql("select 1 -- rollback").is_ok());
        assert!(assert_safe_readonly_sql("rollback; drop table users").is_err());
        assert!(
            assert_safe_readonly_sql("set session characteristics as transaction read write")
                .is_err()
        );
        assert!(assert_safe_readonly_sql("set local statement_timeout = 1").is_err());
    }

    #[test]
    fn explain_validation_accepts_one_query_statement() {
        assert_eq!(
            explainable_statement("select * from users; -- ok").unwrap(),
            "select * from users"
        );
        assert_eq!(
            explainable_statement("update users set name = 'a' where id = 1 returning id").unwrap(),
            "update users set name = 'a' where id = 1 returning id"
        );
        assert!(explainable_statement("select 1; select 2").is_err());
        assert!(explainable_statement("create table users(id int)").is_err());
        assert!(explainable_statement("set local statement_timeout = 1; select 1").is_err());
    }

    #[test]
    fn schema_digest_checksum_ignores_database_identity() {
        let tables = vec![SchemaDigestTable {
            schema: "public".to_string(),
            name: "users".to_string(),
            relation_kind: "table".to_string(),
            columns: vec![SchemaDigestColumn {
                name: "id".to_string(),
                data_type: "integer".to_string(),
                nullable: false,
                default_expression: None,
                generated_expression: None,
            }],
            constraints: Vec::new(),
            indexes: vec![SchemaDigestIndex {
                name: "users_pkey".to_string(),
                definition_hash: "abc123".to_string(),
            }],
            view_definition_hash: None,
        }];
        let extensions = vec![SchemaDigestExtension {
            name: "plpgsql".to_string(),
            version: "1.0".to_string(),
        }];

        let first = SchemaDigestOutput {
            database_id: "db_a".to_string(),
            database_name: "sandbox_a".to_string(),
            digest_version: SCHEMA_DIGEST_VERSION,
            checksum: schema_digest_checksum(&tables, &extensions).unwrap(),
            table_count: 1,
            relation_counts: relation_counts_for_digest_tables(&tables),
            column_count: 1,
            constraint_count: 0,
            index_count: 1,
            extension_count: 1,
            tables: tables.clone(),
            extensions: extensions.clone(),
        };
        let second = SchemaDigestOutput {
            database_id: "db_b".to_string(),
            database_name: "sandbox_b".to_string(),
            digest_version: SCHEMA_DIGEST_VERSION,
            checksum: schema_digest_checksum(&tables, &extensions).unwrap(),
            table_count: 1,
            relation_counts: relation_counts_for_digest_tables(&tables),
            column_count: 1,
            constraint_count: 0,
            index_count: 1,
            extension_count: 1,
            tables,
            extensions,
        };

        assert_eq!(first.checksum, second.checksum);
    }

    #[test]
    fn schema_diff_reports_table_column_index_and_extension_changes() {
        let mut before = test_digest("before");
        let mut after = test_digest("after");
        after.tables[0].columns.push(SchemaDigestColumn {
            name: "email".to_string(),
            data_type: "text".to_string(),
            nullable: false,
            default_expression: None,
            generated_expression: None,
        });
        after.tables[0].indexes[0].definition_hash = "changed".to_string();
        after.tables.push(SchemaDigestTable {
            schema: "public".to_string(),
            name: "posts".to_string(),
            relation_kind: "table".to_string(),
            columns: vec![SchemaDigestColumn {
                name: "id".to_string(),
                data_type: "integer".to_string(),
                nullable: false,
                default_expression: None,
                generated_expression: None,
            }],
            constraints: Vec::new(),
            indexes: Vec::new(),
            view_definition_hash: None,
        });
        after.extensions[0].version = "2.0".to_string();
        after.checksum = "after-checksum".to_string();

        let diff = diff_schema_digests(&before, &after).unwrap();

        assert!(diff.changed);
        assert_eq!(diff.added_tables, ["public.posts"]);
        assert_eq!(diff.changed_extensions, ["plpgsql"]);
        assert_eq!(diff.changed_tables.len(), 1);
        assert_eq!(diff.changed_tables[0].table, "public.users");
        assert_eq!(diff.changed_tables[0].added_columns, ["email"]);
        assert_eq!(diff.changed_tables[0].changed_indexes, ["users_pkey"]);

        before.checksum = after.checksum.clone();
        let unchanged = diff_schema_digests(&before, &before).unwrap();
        assert!(!unchanged.changed);
        assert!(unchanged.changed_tables.is_empty());
    }

    #[test]
    fn schema_diff_reports_constraint_and_view_definition_changes() {
        let mut before = test_digest("before");
        before.tables[0].constraints.push(SchemaDigestConstraint {
            name: "users_email_check".to_string(),
            constraint_type: "check".to_string(),
            definition_hash: "original-check".to_string(),
            update_action: None,
            delete_action: None,
        });

        let mut after = before.clone();
        after.tables[0].constraints[0].definition_hash = "changed-check".to_string();
        after.tables[0].constraints.push(SchemaDigestConstraint {
            name: "users_account_fk".to_string(),
            constraint_type: "foreign_key".to_string(),
            definition_hash: "new-fk".to_string(),
            update_action: Some("NO ACTION".to_string()),
            delete_action: Some("CASCADE".to_string()),
        });
        after.checksum = "after-constraint-checksum".to_string();

        let constraint_diff = diff_schema_digests(&before, &after).unwrap();

        assert!(constraint_diff.changed);
        assert_eq!(constraint_diff.changed_tables.len(), 1);
        assert_eq!(
            constraint_diff.changed_tables[0].added_constraints,
            ["users_account_fk"]
        );
        assert_eq!(
            constraint_diff.changed_tables[0].changed_constraints,
            ["users_email_check"]
        );

        let mut before = test_digest("before");
        before.tables[0].relation_kind = "view".to_string();
        before.tables[0].view_definition_hash = Some("view-v1".to_string());
        let mut after = before.clone();
        after.tables[0].view_definition_hash = Some("view-v2".to_string());
        after.checksum = "after-view-checksum".to_string();

        let view_diff = diff_schema_digests(&before, &after).unwrap();

        assert!(view_diff.changed);
        assert_eq!(view_diff.changed_tables.len(), 1);
        assert!(view_diff.changed_tables[0].view_definition_changed);
        assert!(view_diff.changed_tables[0].changed_columns.is_empty());
        assert!(view_diff.changed_tables[0].changed_indexes.is_empty());
    }

    #[test]
    fn schema_diff_rejects_mismatched_digest_versions() {
        let before = test_digest("before");
        let mut after = test_digest("after");
        after.digest_version = SCHEMA_DIGEST_VERSION + 1;

        let error = diff_schema_digests(&before, &after).unwrap_err();

        assert!(error.to_string().contains("schema digest versions differ"));
    }

    #[test]
    fn schema_diff_base_digest_accepts_serialized_schema_digest_response() {
        let digest = test_digest("base");
        let raw = serde_json::to_string(&digest).unwrap();
        let parsed = serde_json::from_value::<SchemaDiffBaseDigest>(json!(raw)).unwrap();

        assert_eq!(parsed.into_schema_digest().unwrap(), digest);
    }

    #[test]
    fn unknown_postgres_version_mentions_configured_profile_and_managed_local_repair() {
        let manager = PostgresSandboxManager::new(test_config());
        let error = manager.resolve_profile(None, Some("18")).unwrap_err();
        let message = format!("{error:#}");

        assert!(message.contains("postgresVersion 18"));
        assert!(message.contains("default"));
        assert!(message.contains("setup --client"));
        assert!(message.contains("without --admin-url"));
    }

    #[test]
    fn list_profiles_reports_version_tool_count_and_restart_note() {
        let manager = PostgresSandboxManager::new(test_config());
        let output = manager
            .list_profiles(ListProfilesInput {
                include_discovered_local: Some(false),
            })
            .unwrap();

        assert_eq!(output.server_version, crate::VERSION);
        assert_eq!(output.tool_count, PUBLIC_MCP_TOOL_COUNT);
        assert!(output
            .restart_required_after_setup_note
            .contains("After setup or upgrade"));
        assert!(output
            .hints
            .iter()
            .any(|hint| hint.contains("restart the MCP client")));
        assert!(output
            .hints
            .iter()
            .any(|hint| hint.contains("without --admin-url")));
        let profile = serde_json::to_value(&output.profiles[0]).unwrap();
        assert!(profile.get("serverVersion").is_none());
    }

    #[test]
    fn all_version_request_accepts_flag_or_star() {
        assert!(all_versions_requested(None, Some(true)));
        assert!(all_versions_requested(Some("*"), None));
        assert!(all_versions_requested(Some(" * "), Some(false)));
        assert!(!all_versions_requested(Some("18"), Some(false)));
        assert!(!all_versions_requested(None, None));
    }

    #[test]
    fn all_version_profile_scan_skips_deferred_managed_local_without_starting() {
        let mut config = test_config();
        config.managed_local.enabled = true;
        config.profiles.push(SandboxProfile {
            name: "local-pg123456".to_string(),
            admin_url: DEFERRED_LOCAL_ADMIN_URL.to_string(),
            database_prefix: "pgsandbox".to_string(),
            default_ttl_minutes: 240,
            max_ttl_minutes: 1440,
            allow_external_admin_url: false,
            allowed_admin_hosts: Vec::new(),
            max_active_databases_per_owner: None,
            postgres_version: Some("123456".to_string()),
            managed_local: true,
        });
        let manager = PostgresSandboxManager::new(config);

        let profiles = manager.profiles_for_all_version_operations().unwrap();
        let names = profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"default"));
        assert!(!names.contains(&"local-pg123456"));
    }

    #[tokio::test]
    async fn all_version_list_collects_profile_connection_failures() {
        let mut config = test_config();
        config.profiles[0].admin_url =
            "postgres://postgres:secret@127.0.0.1:1/postgres?sslmode=disable".to_string();
        let manager = PostgresSandboxManager::new(config);

        let output = manager
            .list_databases(ListDatabasesInput {
                profile: None,
                postgres_version: None,
                include_all_versions: Some(true),
                owner: None,
            })
            .await
            .unwrap();

        assert_eq!(output.scope, "allVersions");
        assert!(output.databases.is_empty());
        assert_eq!(output.failures.len(), 1);
        assert_eq!(output.failures[0]["profile"], "default");
        assert_eq!(output.failures[0]["category"], "profile_unavailable");
        assert!(!output.failures[0]["message"]
            .as_str()
            .unwrap()
            .contains("secret"));
    }

    #[tokio::test]
    async fn all_version_cleanup_collects_profile_connection_failures() {
        let mut config = test_config();
        config.profiles[0].admin_url =
            "postgres://postgres:secret@127.0.0.1:1/postgres?sslmode=disable".to_string();
        let manager = PostgresSandboxManager::new(config);

        let output = manager
            .cleanup_expired(CleanupExpiredInput {
                profile: None,
                postgres_version: None,
                include_all_versions: Some(true),
                dry_run: Some(true),
            })
            .await
            .unwrap();

        let failures = output.failures.unwrap();
        assert_eq!(output.scope, "allVersions");
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0]["profile"], "default");
        assert_eq!(failures[0]["category"], "profile_unavailable");
        assert!(!failures[0]["message"].as_str().unwrap().contains("secret"));
    }

    #[test]
    fn unscoped_database_id_is_cross_profile_lookup_candidate() {
        assert!(selector_is_unscoped_database_id(
            None,
            None,
            Some(&"db-id".to_string()),
            None
        ));
        assert!(!selector_is_unscoped_database_id(
            Some(&"local-pg18".to_string()),
            None,
            Some(&"db-id".to_string()),
            None
        ));
        assert!(!selector_is_unscoped_database_id(
            None,
            Some(&"18".to_string()),
            Some(&"db-id".to_string()),
            None
        ));
        assert!(!selector_is_unscoped_database_id(
            None,
            None,
            Some(&"db-id".to_string()),
            Some(&"pgsandbox_name".to_string())
        ));
    }

    #[test]
    fn clone_downgrade_preflight_flags_newer_source() {
        assert!(clone_downgrade_error("18", "16").is_some());
        assert!(clone_downgrade_error("18", "18").is_none());
        assert!(clone_downgrade_error("16", "18").is_none());
        assert_eq!(postgres_major_from_server_version("18.4").unwrap(), "18");
        assert_eq!(postgres_major_from_server_version("16").unwrap(), "16");
    }

    #[test]
    fn workflow_errors_include_agent_branching_category() {
        let template = workflow_error("template_not_found", "missing", None);
        let repo_command = workflow_error("repo_command_failed", "failed", None);
        let validation = workflow_error("missing_schema_change_command", "missing", None);
        let unsafe_command = workflow_error("unsafe_command", "unsafe", None);

        assert_eq!(template.category, "template_not_found");
        assert_eq!(repo_command.category, "command_failed");
        assert_eq!(validation.category, "validation");
        assert_eq!(unsafe_command.category, "validation");
    }

    #[test]
    fn schema_snapshot_workflow_error_classifies_timeouts() {
        let timeout = schema_snapshot_workflow_error(anyhow::anyhow!(
            "schema_operation_timeout: schema digest exceeded 30 seconds"
        ));
        let failed = schema_snapshot_workflow_error(anyhow::anyhow!("catalog query failed"));

        assert_eq!(timeout.code, "schema_snapshot_timeout");
        assert_eq!(timeout.category, "workflow");
        assert_eq!(failed.code, "schema_snapshot_failed");
        assert_eq!(failed.category, "workflow");
    }

    #[test]
    fn deferred_managed_local_profile_summary_does_not_show_placeholder_url() {
        let mut config = test_config();
        config.default_profile = "local".to_string();
        config.managed_local.enabled = true;
        config.profiles[0].name = "local".to_string();
        config.profiles[0].managed_local = true;
        config.profiles[0].admin_url = DEFERRED_LOCAL_ADMIN_URL.to_string();
        let manager = PostgresSandboxManager::new(config);

        let output = manager
            .list_profiles(ListProfilesInput {
                include_discovered_local: Some(false),
            })
            .unwrap();

        assert_eq!(
            output.profiles[0].admin_url,
            "(managed local; starts on demand)"
        );
    }

    #[test]
    fn owner_quota_counts_only_unexpired_active_databases() {
        let sql = active_owner_quota_sql().unwrap();

        assert!(sql.contains("deleted_at IS NULL"));
        assert!(sql.contains("expires_at > now()"));
    }

    #[test]
    fn query_mode_detects_row_producing_sql() {
        assert!(matches!(query_mode("select 1"), QueryMode::Cursor));
        assert!(matches!(query_mode("table users"), QueryMode::Cursor));
        assert!(matches!(
            query_mode("select 'returning' as word"),
            QueryMode::Cursor
        ));
        assert!(matches!(
            query_mode("insert into users(name) values ('a') returning id"),
            QueryMode::TypedRows
        ));
        assert!(matches!(
            query_mode("update users set status = 'returning' where id = 1"),
            QueryMode::Simple
        ));
        assert!(matches!(
            query_mode("update users set status = $$returning$$ where id = 1"),
            QueryMode::Simple
        ));
        assert!(matches!(
            query_mode("update users set status = 'done' -- returning\nwhere id = 1"),
            QueryMode::Simple
        ));
        assert!(matches!(
            query_mode("show server_version"),
            QueryMode::TypedRows
        ));
        assert!(matches!(
            query_mode("create table users(id int)"),
            QueryMode::Simple
        ));
    }

    #[test]
    fn limits_top_level_dml_returning_queries() {
        let limited = dml_returning_limit_sql(
            "insert into users(name) values ('a') returning id, name;",
            100,
        )
        .unwrap();

        assert!(limited.starts_with("WITH pgsandbox_limited_returning_"));
        assert!(limited.contains(" AS (\ninsert into users"));
        assert!(limited.ends_with("LIMIT 101"));
        let with_trailing_comment = dml_returning_limit_sql(
            "insert into users(name) values ('a') returning id -- newly created row",
            100,
        )
        .unwrap();
        assert!(with_trailing_comment.contains("-- newly created row\n) SELECT"));
        assert_ne!(
            returning_limit_alias("insert into users(name) values ('a') returning id"),
            returning_limit_alias("insert into users(name) values ('b') returning id")
        );
        assert!(dml_returning_limit_sql("select 'returning' as word", 100).is_none());
        assert!(
            dml_returning_limit_sql("update users set status = 'returning' where id = 1", 100)
                .is_none()
        );
        assert!(dml_returning_limit_sql(
            "/* returning */ update users set status = 'done' where id = 1",
            100
        )
        .is_none());
    }

    #[test]
    fn parses_postgres_sslmodes() {
        assert_eq!(
            ssl_mode_from_url("postgres://postgres@localhost/postgres").unwrap(),
            SslMode::Prefer
        );
        assert_eq!(
            ssl_mode_from_url("postgres://postgres@localhost/postgres?sslmode=disable").unwrap(),
            SslMode::Disable
        );
        assert_eq!(
            ssl_mode_from_url("postgres://postgres@localhost/postgres?sslmode=require").unwrap(),
            SslMode::Require
        );
        assert_eq!(
            ssl_mode_from_url("postgres://postgres@localhost/postgres?sslmode=verify-ca").unwrap(),
            SslMode::VerifyCa
        );
        assert!(ssl_mode_from_url("postgres://postgres@localhost/postgres?sslmode=nope").is_err());
    }

    #[test]
    fn role_passwords_are_encrypted_at_rest() {
        let profile = SandboxProfile {
            name: "default".to_string(),
            admin_url: "postgres://postgres:secret@localhost/postgres".to_string(),
            database_prefix: "pgsandbox".to_string(),
            default_ttl_minutes: 15,
            max_ttl_minutes: 60,
            allow_external_admin_url: false,
            allowed_admin_hosts: Vec::new(),
            max_active_databases_per_owner: None,
            postgres_version: None,
            managed_local: false,
        };
        let stored = protect_role_password("sandbox-secret", &profile).unwrap();

        assert_ne!(stored, "sandbox-secret");
        assert!(stored.starts_with("v1:"));
        assert_eq!(
            unprotect_role_password(&stored, &profile).unwrap(),
            "sandbox-secret"
        );
    }

    #[test]
    fn pg_tool_connection_uses_environment_for_credentials() {
        let connection = pg_tool_connection_from_url(
            "postgres://clone%2Duser:p%40ss%2Fword@db.example.com:6543/prod%2Dmain?sslmode=require",
        )
        .unwrap();

        assert_eq!(connection.database, "prod-main");
        assert_eq!(
            connection.env.get("PGHOST").map(String::as_str),
            Some("db.example.com")
        );
        assert_eq!(
            connection.env.get("PGPORT").map(String::as_str),
            Some("6543")
        );
        assert_eq!(
            connection.env.get("PGUSER").map(String::as_str),
            Some("clone-user")
        );
        assert_eq!(
            connection.env.get("PGPASSWORD").map(String::as_str),
            Some("p@ss/word")
        );
        assert_eq!(
            connection.env.get("PGSSLMODE").map(String::as_str),
            Some("require")
        );
    }

    #[test]
    fn pg_tool_connection_forwards_tls_certificate_parameters() {
        let connection = pg_tool_connection_from_url(
            "postgres://postgres@db.example.com/prod?sslmode=verify-full&sslrootcert=%2Fcerts%2Fca.pem&sslcert=%2Fcerts%2Fclient.pem&sslkey=%2Fcerts%2Fclient.key",
        )
        .unwrap();

        assert_eq!(
            connection.env.get("PGSSLMODE").map(String::as_str),
            Some("verify-full")
        );
        assert_eq!(
            connection.env.get("PGSSLROOTCERT").map(String::as_str),
            Some("/certs/ca.pem")
        );
        assert_eq!(
            connection.env.get("PGSSLCERT").map(String::as_str),
            Some("/certs/client.pem")
        );
        assert_eq!(
            connection.env.get("PGSSLKEY").map(String::as_str),
            Some("/certs/client.key")
        );
    }

    #[test]
    fn pg_tool_connection_rejects_urls_without_database_names() {
        assert!(pg_tool_connection_from_url("postgres://postgres@localhost").is_err());
        assert!(pg_tool_connection_from_url("https://postgres@localhost/postgres").is_err());
    }

    #[test]
    fn clone_dump_and_restore_args_do_not_include_connection_urls() {
        let source = pg_tool_connection_from_url(
            "postgres://source:secret@localhost/source_db?sslmode=require",
        )
        .unwrap();
        let target = pg_tool_connection_from_url(
            "postgres://target:target-secret@localhost/target_db?sslmode=require",
        )
        .unwrap();

        let dump_args = pg_dump_args(&source.database, false);
        let schema_only_dump_args = pg_dump_args(&source.database, true);
        let restore_args = pg_restore_args(&target.database);
        let joined_args = dump_args
            .iter()
            .chain(schema_only_dump_args.iter())
            .chain(restore_args.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");

        assert!(!joined_args.contains("postgres://"));
        assert!(!joined_args.contains("secret"));
        assert!(dump_args.contains(&"--format=custom".to_string()));
        assert!(dump_args.contains(&"--no-owner".to_string()));
        assert!(dump_args.contains(&"--no-privileges".to_string()));
        assert!(schema_only_dump_args.contains(&"--schema-only".to_string()));
        assert!(restore_args.contains(&"--single-transaction".to_string()));
        assert!(restore_args.contains(&"--exit-on-error".to_string()));
        assert!(restore_args.contains(&"target_db".to_string()));
    }

    #[test]
    fn template_file_args_do_not_include_connection_urls() {
        let dump_path = PathBuf::from("/tmp/pgsandbox-template.dump");
        let dump_args = pg_dump_file_args("source_db", &dump_path, false);
        let restore_args = pg_restore_file_args("target_db", &dump_path);
        let joined_args = dump_args
            .iter()
            .chain(restore_args.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");

        assert!(!joined_args.contains("postgres://"));
        assert!(dump_args.contains(&"--file".to_string()));
        assert!(dump_args.contains(&dump_path.display().to_string()));
        assert!(restore_args.contains(&"--single-transaction".to_string()));
        assert!(restore_args.contains(&dump_path.display().to_string()));
    }

    #[test]
    fn validates_artifact_names_for_local_files() {
        assert_eq!(
            validate_artifact_name("seeded_state.v1", "templateName").unwrap(),
            "seeded_state.v1"
        );
        assert!(validate_artifact_name("../prod", "templateName").is_err());
        assert!(validate_artifact_name("nested/name", "templateName").is_err());
        assert!(validate_artifact_name("", "templateName").is_err());
        assert!(snapshot_paths("local", "../prod", "before").is_err());
    }

    #[test]
    fn mask_connection_string_never_returns_unparseable_input() {
        let masked = mask_connection_string("postgres://sandbox:secret@localhost/app");

        assert!(masked.contains("****"));
        assert!(!masked.contains("secret"));
        assert_eq!(
            mask_connection_string("not a postgres url with password=secret"),
            "<unparseable connection string>"
        );
    }

    #[test]
    fn creation_outputs_serialize_only_redacted_connection_strings() {
        let expires_at = Utc::now();
        let connection_string = "postgres://role:secret@localhost:5432/sandbox";

        let created = serde_json::to_value(CreateDatabaseOutput {
            database_id: "db-id".to_string(),
            profile: "default".to_string(),
            database_name: "sandbox".to_string(),
            role_name: "sandbox_role".to_string(),
            expires_at,
            connection_string: connection_string.to_string(),
            connection_string_redacted: mask_connection_string(connection_string),
        })
        .unwrap();
        assert!(created.get("connectionString").is_none());
        assert_eq!(
            created
                .get("connectionStringRedacted")
                .and_then(Value::as_str),
            Some("postgres://role:****@localhost:5432/sandbox")
        );

        let cloned = serde_json::to_value(CloneDatabaseOutput {
            database_id: "db-id".to_string(),
            profile: "default".to_string(),
            database_name: "sandbox".to_string(),
            role_name: "sandbox_role".to_string(),
            expires_at,
            connection_string: connection_string.to_string(),
            connection_string_redacted: mask_connection_string(connection_string),
            source: "external".to_string(),
            schema_only: false,
        })
        .unwrap();
        assert!(cloned.get("connectionString").is_none());
        assert_eq!(
            cloned
                .get("connectionStringRedacted")
                .and_then(Value::as_str),
            Some("postgres://role:****@localhost:5432/sandbox")
        );
    }

    #[test]
    fn template_restore_envelope_does_not_serialize_full_connection_string() {
        let connection_string = "postgres://role:secret@localhost:5432/sandbox";
        let created_sandbox = CreateSandboxFromTemplateOutput {
            database_id: "db-id".to_string(),
            profile: "default".to_string(),
            database_name: "sandbox".to_string(),
            role_name: "sandbox_role".to_string(),
            expires_at: Utc::now(),
            connection_string: connection_string.to_string(),
            connection_string_redacted: mask_connection_string(connection_string),
            template_name: "seeded".to_string(),
        };
        let mut envelope = workflow_success(
            "Sandbox created from template `seeded`.",
            None,
            Vec::new(),
            Vec::new(),
            created_sandbox.clone(),
        );
        envelope.created_sandbox = Some(created_sandbox);

        let payload = serde_json::to_value(envelope).unwrap();
        assert!(payload
            .get("result")
            .and_then(|result| result.get("connectionString"))
            .is_none());
        assert!(payload
            .get("createdSandbox")
            .and_then(|result| result.get("connectionString"))
            .is_none());
        assert!(!payload.to_string().contains("secret"));
    }

    #[test]
    fn explicit_connection_string_lookup_serializes_full_connection_string() {
        let connection_string = "postgres://role:secret@localhost:5432/sandbox";
        let output = serde_json::to_value(ConnectionStringOutput {
            database_id: "db-id".to_string(),
            database_name: "sandbox".to_string(),
            expires_at: Utc::now(),
            connection_string: connection_string.to_string(),
            connection_string_redacted: mask_connection_string(connection_string),
        })
        .unwrap();

        assert_eq!(
            output.get("connectionString").and_then(Value::as_str),
            Some(connection_string)
        );
        assert_eq!(
            output
                .get("connectionStringRedacted")
                .and_then(Value::as_str),
            Some("postgres://role:****@localhost:5432/sandbox")
        );
    }

    #[test]
    fn creation_outputs_debug_redacts_full_connection_strings() {
        let expires_at = Utc::now();
        let connection_string = "postgres://role:secret@localhost:5432/sandbox";

        let created = CreateDatabaseOutput {
            database_id: "db-id".to_string(),
            profile: "default".to_string(),
            database_name: "sandbox".to_string(),
            role_name: "sandbox_role".to_string(),
            expires_at,
            connection_string: connection_string.to_string(),
            connection_string_redacted: mask_connection_string(connection_string),
        };
        let cloned = CloneDatabaseOutput {
            database_id: "db-id".to_string(),
            profile: "default".to_string(),
            database_name: "sandbox".to_string(),
            role_name: "sandbox_role".to_string(),
            expires_at,
            connection_string: connection_string.to_string(),
            connection_string_redacted: mask_connection_string(connection_string),
            source: "external".to_string(),
            schema_only: false,
        };
        let restored = CreateSandboxFromTemplateOutput {
            database_id: "db-id".to_string(),
            profile: "default".to_string(),
            database_name: "sandbox".to_string(),
            role_name: "sandbox_role".to_string(),
            expires_at,
            connection_string: connection_string.to_string(),
            connection_string_redacted: mask_connection_string(connection_string),
            template_name: "seeded".to_string(),
        };

        for debug_output in [
            format!("{created:?}"),
            format!("{cloned:?}"),
            format!("{restored:?}"),
        ] {
            assert!(!debug_output.contains("secret"));
            assert!(!debug_output.contains(connection_string));
            assert!(debug_output.contains("connection_string"));
            assert!(debug_output.contains("<redacted>"));
        }
    }

    #[test]
    fn writes_generic_secret_free_project_config() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();

        let config = RepoProjectConfig {
            migration_command: Some(vec![
                "npm".to_string(),
                "run".to_string(),
                "migrate".to_string(),
            ]),
            seed_command: Some(vec![
                "npm".to_string(),
                "run".to_string(),
                "seed".to_string(),
            ]),
            database_url_env: "DATABASE_URL".to_string(),
            postgres_version: None,
            prepared_at: Utc::now(),
        };
        let path = write_repo_project_config(repo, &config).unwrap();
        let raw = std::fs::read_to_string(path).unwrap();

        assert!(!raw.contains("\"framework\""));
        assert!(raw.contains("\"migrationCommand\""));
        assert!(raw.contains("\"npm\""));
        assert!(!raw.contains("postgres://"));
        assert!(!raw.contains("secret"));
    }

    #[tokio::test]
    async fn prepare_for_repo_preserves_existing_commands_when_updating_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        let config = RepoProjectConfig {
            migration_command: Some(vec![
                "npm".to_string(),
                "run".to_string(),
                "migrate".to_string(),
            ]),
            seed_command: Some(vec![
                "npm".to_string(),
                "run".to_string(),
                "seed".to_string(),
            ]),
            database_url_env: "DATABASE_URL".to_string(),
            postgres_version: Some("16".to_string()),
            prepared_at: Utc::now(),
        };
        write_repo_project_config(repo, &config).unwrap();
        let manager = PostgresSandboxManager::new(test_config());

        let output = manager
            .prepare_for_repo(PrepareForRepoInput {
                repo_path: repo.display().to_string(),
                profile: None,
                postgres_version: Some("17".to_string()),
                database_id: None,
                database_name: None,
                migration_command: None,
                seed_command: None,
            })
            .await
            .unwrap();
        let updated = read_repo_project_config(repo).unwrap().unwrap();

        assert!(output.ok);
        assert!(output.result.unwrap().migration_command_configured);
        assert_eq!(updated.postgres_version.as_deref(), Some("17"));
        assert_eq!(
            updated.migration_command.as_deref(),
            Some(["npm".to_string(), "run".to_string(), "migrate".to_string()].as_slice())
        );
        assert_eq!(
            updated.seed_command.as_deref(),
            Some(["npm".to_string(), "run".to_string(), "seed".to_string()].as_slice())
        );
    }

    #[test]
    fn infers_postgres_version_from_compose_postgres_image() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        std::fs::write(
            repo.join("compose.yaml"),
            r#"
services:
  db:
    image: postgres:17.2
"#,
        )
        .unwrap();

        let inference = infer_repo_postgres_version(repo).unwrap().unwrap();

        assert_eq!(inference.version, "17");
        assert_eq!(inference.source, "compose.yaml services.db.image");
    }

    #[test]
    fn infers_postgres_version_from_timescale_pg_tag() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        std::fs::write(
            repo.join("compose.yaml"),
            r#"
services:
  db:
    image: timescaledb/timescaledb:2.11.2-pg16
"#,
        )
        .unwrap();

        let inference = infer_repo_postgres_version(repo).unwrap().unwrap();

        assert_eq!(inference.version, "16");
        assert_eq!(inference.source, "compose.yaml services.db.image");
    }

    #[test]
    fn ignores_non_postgres_images_with_postgres_substrings() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        std::fs::write(
            repo.join("compose.yaml"),
            r#"
services:
  api:
    image: postgrest/postgrest:10.1
  exporter:
    image: prometheuscommunity/postgres-exporter:v0.14.0
  db:
    image: postgres:16
"#,
        )
        .unwrap();

        let inference = infer_repo_postgres_version(repo).unwrap().unwrap();

        assert_eq!(inference.version, "16");
        assert_eq!(inference.source, "compose.yaml services.db.image");
        assert!(postgres_version_from_image("postgres-exporter:0.14").is_none());
    }

    #[test]
    fn infers_postgres_version_from_devcontainer_compose_image() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        let devcontainer = repo.join(".devcontainer");
        std::fs::create_dir_all(&devcontainer).unwrap();
        std::fs::write(
            devcontainer.join("devcontainer.json"),
            r#"
{
  "name": "app",
  "features": {},
  "customizations": {},
  "image": "mcr.microsoft.com/devcontainers/rust:1",
  "services": {
    "db": {
      "image": "postgis/postgis:16-3.4"
    }
  }
}
"#,
        )
        .unwrap();

        let inference = infer_repo_postgres_version(repo).unwrap().unwrap();

        assert_eq!(inference.version, "16");
        assert_eq!(
            inference.source,
            ".devcontainer/devcontainer.json services.db.image"
        );
    }

    #[test]
    fn repo_postgres_version_prefers_explicit_input_then_project_config() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        let config = RepoProjectConfig {
            migration_command: Some(vec![
                "npm".to_string(),
                "run".to_string(),
                "migrate".to_string(),
            ]),
            seed_command: None,
            database_url_env: "DATABASE_URL".to_string(),
            postgres_version: Some("16".to_string()),
            prepared_at: Utc::now(),
        };
        write_repo_project_config(repo, &config).unwrap();

        let explicit = resolve_repo_postgres_version(repo, Some("17".to_string())).unwrap();
        let configured = resolve_repo_postgres_version(repo, None).unwrap();

        assert_eq!(explicit.version.as_deref(), Some("17"));
        assert_eq!(explicit.source.as_deref(), Some("input postgresVersion"));
        assert_eq!(configured.version.as_deref(), Some("16"));
        assert_eq!(
            configured.source.as_deref(),
            Some(".pgsandbox/project.json postgresVersion")
        );
    }

    #[test]
    fn migration_commands_are_generic_bounded_non_shell_argv() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();

        let django = resolve_repo_schema_command(
            repo,
            Some(vec![
                "npm".to_string(),
                "run".to_string(),
                "migrate".to_string(),
            ]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(django[2], "migrate");

        let shell = resolve_repo_schema_command(
            repo,
            Some(vec![
                "bash".to_string(),
                "-lc".to_string(),
                "npm run migrate".to_string(),
            ]),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(shell.code, "unsafe_command");

        let multiline_shell = resolve_repo_schema_command(
            repo,
            Some(vec![
                "bash".to_string(),
                "-lc".to_string(),
                "set -euo pipefail\npsql \"$DATABASE_URL\" -Atc \"SELECT 1\"".to_string(),
            ]),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(multiline_shell.code, "unsafe_command");
        assert!(multiline_shell
            .message
            .contains("cannot invoke a shell or command launcher"));

        let env_shell = resolve_repo_schema_command(
            repo,
            Some(vec![
                "env".to_string(),
                "bash".to_string(),
                "-c".to_string(),
                "npm run migrate".to_string(),
            ]),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(env_shell.code, "unsafe_command");

        let sudo_shell = resolve_repo_schema_command(
            repo,
            Some(vec![
                "sudo".to_string(),
                "/bin/sh".to_string(),
                "-c".to_string(),
                "npm run migrate".to_string(),
            ]),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(sudo_shell.code, "unsafe_command");

        let launcher_without_shell = resolve_repo_schema_command(
            repo,
            Some(vec![
                "nsenter".to_string(),
                "--target".to_string(),
                "1".to_string(),
                "npm".to_string(),
                "run".to_string(),
                "migrate".to_string(),
            ]),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(launcher_without_shell.code, "unsafe_command");

        let alembic = resolve_repo_schema_command(
            repo,
            Some(vec![
                "alembic".to_string(),
                "upgrade".to_string(),
                "head".to_string(),
            ]),
        )
        .unwrap();
        assert!(alembic.is_ok());

        let prisma = resolve_repo_schema_command(
            repo,
            Some(vec![
                "npx".to_string(),
                "prisma".to_string(),
                "migrate".to_string(),
                "deploy".to_string(),
            ]),
        )
        .unwrap();
        assert!(prisma.is_ok());

        let rails = resolve_repo_schema_command(
            repo,
            Some(vec![
                "bundle".to_string(),
                "exec".to_string(),
                "rails".to_string(),
                "db:migrate".to_string(),
            ]),
        )
        .unwrap();
        assert!(rails.is_ok());

        let psql_file = resolve_repo_schema_command(
            repo,
            Some(vec![
                "psql".to_string(),
                "$DATABASE_URL".to_string(),
                "-f".to_string(),
                "schema.sql".to_string(),
            ]),
        )
        .unwrap();
        assert!(psql_file.is_ok());

        let missing = resolve_repo_schema_command(repo, None)
            .unwrap()
            .unwrap_err();
        assert_eq!(missing.code, "missing_schema_change_command");

        let empty = resolve_repo_schema_command(repo, Some(Vec::new()))
            .unwrap()
            .unwrap_err();
        assert_eq!(empty.code, "unclear_command");
        assert!(empty.message.contains("0 argv parts"));
        assert!(empty.message.contains("maximum 16"));
        assert!(!empty.message.contains("longest part"));

        let oversized = resolve_repo_schema_command(repo, Some(vec!["x".repeat(257)]))
            .unwrap()
            .unwrap_err();
        assert_eq!(oversized.code, "unclear_command");
        assert!(oversized.message.contains("1 argv part"));
        assert!(oversized.message.contains("longest part is 257 bytes"));
        assert!(oversized.hint.unwrap().contains("maximum 256 bytes"));
    }

    #[test]
    fn unscoped_database_name_uses_cross_profile_lookup_policy() {
        let name = "pgsandbox_app_123".to_string();
        let id = "abc".to_string();

        assert!(selector_is_unscoped_database_name(
            None,
            None,
            None,
            Some(&name)
        ));
        assert!(!selector_is_unscoped_database_name(
            Some(&"local".to_string()),
            None,
            None,
            Some(&name)
        ));
        assert!(!selector_is_unscoped_database_name(
            None,
            None,
            Some(&id),
            Some(&name)
        ));
    }

    #[test]
    fn seed_command_requires_explicit_input_or_project_config() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        let missing = resolve_seed_command(repo, None).unwrap().unwrap_err();

        assert_eq!(missing.code, "missing_seed_command");

        let explicit = resolve_seed_command(
            repo,
            Some(vec![
                "npm".to_string(),
                "run".to_string(),
                "seed".to_string(),
            ]),
        )
        .unwrap();
        assert!(explicit.is_ok());
    }

    #[test]
    fn command_env_injects_database_url_and_pg_vars() {
        let env = database_command_env(
            "postgres://sandbox:p%40ss@localhost:65432/app_db?sslmode=disable",
        )
        .unwrap();

        assert_eq!(env.get("PGDATABASE").map(String::as_str), Some("app_db"));
        assert_eq!(env.get("PGUSER").map(String::as_str), Some("sandbox"));
        assert_eq!(env.get("PGPASSWORD").map(String::as_str), Some("p@ss"));
        assert_eq!(
            env.get("DATABASE_URL").map(String::as_str),
            Some("postgres://sandbox:p%40ss@localhost:65432/app_db?sslmode=disable")
        );
    }

    #[test]
    fn schema_diff_counts_all_changes_but_bounds_output() {
        let from = workflow_test_digest((0..80).map(|index| format!("public.t_{index}")).collect());
        let to = workflow_test_digest((80..160).map(|index| format!("public.t_{index}")).collect());
        let diff = diff_workflow_schema_digests(&from, &to);

        assert_eq!(diff.changed_objects.added, 80);
        assert_eq!(diff.changed_objects.removed, 80);
        assert_eq!(diff.added.len(), MAX_SCHEMA_DIFF_ITEMS);
        assert_eq!(diff.removed.len(), MAX_SCHEMA_DIFF_ITEMS);
        assert!(diff.truncated);
    }

    #[test]
    fn workflow_schema_diff_version_mismatch_is_detectable() {
        let mut snapshot = workflow_test_digest(vec!["public.users".to_string()]);
        let current = workflow_test_digest(vec!["public.users".to_string()]);
        snapshot.digest_version = SCHEMA_DIGEST_VERSION - 1;

        let error =
            workflow_schema_digest_version_mismatch("baseline", &snapshot, &current).unwrap();

        assert_eq!(error.code, "schema_digest_version_mismatch");
        assert_eq!(error.category, "workflow");
        assert!(error.message.contains("baseline"));
        assert!(error.message.contains("v1"));
        assert!(error.message.contains("v2"));
        assert!(error
            .hint
            .unwrap()
            .contains("create_schema_snapshot before diffing"));
        assert!(workflow_schema_digest_version_mismatch("baseline", &current, &current).is_none());
    }

    #[test]
    fn summarizes_tool_stderr_without_splitting_utf8_characters() {
        let stderr = format!("{}éproblem", "a".repeat(3_999));
        let summary = summarize_tool_stderr(stderr.as_bytes());

        assert!(summary.ends_with("..."));
        assert_eq!(summary.len(), 4_002);
        assert!(!summary.contains('é'));
    }

    #[test]
    fn reports_restore_failure_before_dump_sigpipe_failure() {
        let message = clone_tool_failure_message(
            false,
            b"pg_dump: error: could not write to output pipe",
            false,
            b"pg_restore: error: duplicate key value violates unique constraint",
        )
        .unwrap();

        assert!(message.starts_with("pg_restore failed:"));
        assert!(message.contains("duplicate key"));
    }

    #[test]
    fn serializes_common_postgres_arrays_as_json_arrays() {
        let timestamp = DateTime::parse_from_rfc3339("2026-07-01T12:34:56Z")
            .unwrap()
            .with_timezone(&Utc);
        let plain_timestamp = NaiveDate::from_ymd_opt(2026, 7, 1)
            .unwrap()
            .and_hms_opt(12, 34, 56)
            .unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        let uuid = Uuid::parse_str("0f3f2410-ae28-44d2-98c9-09bc42cf12d1").unwrap();

        assert_eq!(
            optional_array_to_json(
                Some(vec![
                    Some("alpha".to_string()),
                    None,
                    Some("beta".to_string())
                ]),
                Value::String
            ),
            json!(["alpha", null, "beta"])
        );
        assert_eq!(
            optional_array_to_json(Some(vec![Some(1_i32), None, Some(3_i32)]), |value| json!(
                value
            )),
            json!([1, null, 3])
        );
        assert_eq!(
            optional_array_to_json(Some(vec![Some(uuid), None]), |value| Value::String(
                value.to_string()
            )),
            json!(["0f3f2410-ae28-44d2-98c9-09bc42cf12d1", null])
        );
        assert_eq!(
            optional_array_to_json(Some(vec![Some(json!({"ok": true})), None]), |value| value),
            json!([{"ok": true}, null])
        );
        assert_eq!(
            optional_array_to_json(Some(vec![Some(timestamp), None]), |value| Value::String(
                value.to_rfc3339()
            )),
            json!(["2026-07-01T12:34:56+00:00", null])
        );
        assert_eq!(
            optional_array_to_json(Some(vec![Some(plain_timestamp), None]), |value| {
                Value::String(value.to_string())
            }),
            json!(["2026-07-01 12:34:56", null])
        );
        assert_eq!(
            optional_array_to_json(Some(vec![Some(date), None]), |value| Value::String(
                value.to_string()
            )),
            json!(["2026-07-01", null])
        );
        assert_eq!(
            optional_array_to_json::<String, _>(None, Value::String),
            Value::Null
        );
    }

    #[test]
    fn maps_common_postgres_array_types_to_json_array_serializers() {
        assert_eq!(
            array_cell_kind(&Type::TEXT_ARRAY),
            Some(ArrayCellKind::Text)
        );
        assert_eq!(
            array_cell_kind(&Type::INT4_ARRAY),
            Some(ArrayCellKind::Int4)
        );
        assert_eq!(
            array_cell_kind(&Type::UUID_ARRAY),
            Some(ArrayCellKind::Uuid)
        );
        assert_eq!(
            array_cell_kind(&Type::JSONB_ARRAY),
            Some(ArrayCellKind::Json)
        );
        assert_eq!(
            array_cell_kind(&Type::DATE_ARRAY),
            Some(ArrayCellKind::Date)
        );
        assert_eq!(
            array_cell_kind(&Type::TIMESTAMP_ARRAY),
            Some(ArrayCellKind::Timestamp)
        );
        assert_eq!(
            array_cell_kind(&Type::TIMESTAMPTZ_ARRAY),
            Some(ArrayCellKind::TimestampTz)
        );
    }

    #[test]
    fn readonly_violation_message_names_mutating_statement() {
        let error = anyhow::anyhow!("db error: cannot execute SELECT in a read-only transaction");
        let message =
            readonly_violation_message("insert into users(name) values ('a') returning id", &error)
                .unwrap();

        assert!(message.contains("readonly=true blocked INSERT statement"));
        assert!(message.contains("Database detail:"));
        assert!(!message.contains("blocked SELECT statement"));
    }

    #[test]
    fn relation_counts_split_tables_views_and_materialized_views() {
        let counts = relation_counts_for_digest_tables(&[
            SchemaDigestTable {
                schema: "public".to_string(),
                name: "users".to_string(),
                relation_kind: "table".to_string(),
                columns: Vec::new(),
                indexes: Vec::new(),
                constraints: Vec::new(),
                view_definition_hash: None,
            },
            SchemaDigestTable {
                schema: "public".to_string(),
                name: "active_users".to_string(),
                relation_kind: "view".to_string(),
                columns: Vec::new(),
                indexes: Vec::new(),
                constraints: Vec::new(),
                view_definition_hash: Some("view-hash".to_string()),
            },
            SchemaDigestTable {
                schema: "public".to_string(),
                name: "daily_rollup".to_string(),
                relation_kind: "materialized_view".to_string(),
                columns: Vec::new(),
                indexes: Vec::new(),
                constraints: Vec::new(),
                view_definition_hash: Some("matview-hash".to_string()),
            },
        ]);

        assert_eq!(counts.tables, 1);
        assert_eq!(counts.views, 1);
        assert_eq!(counts.materialized_views, 1);
        assert_eq!(counts.total(), 3);
    }

    #[test]
    fn generated_column_defaults_are_guarded_in_catalog_queries() {
        let source = include_str!("postgres.rs");
        let pg_get_expr = "pg_get_expr(ad.adbin, ad.adrelid)";
        let guarded_default =
            format!("CASE WHEN a.attgenerated = '' THEN {pg_get_expr} ELSE NULL END");

        assert!(source.matches(&guarded_default).count() >= 4);
        assert!(!source.contains(&format!("{pg_get_expr} AS column_default")));
        assert!(!source.contains(&format!("{pg_get_expr} AS default_expression")));
        assert!(!source.contains(&format!("{pg_get_expr} AS \"columnDefault\"")));
    }

    #[test]
    fn decodes_postgres_numeric_values() {
        assert_eq!(
            decode_pg_numeric(&numeric_raw(1, 0x0000, 2, &[1, 2345, 6700])).unwrap(),
            "12345.67"
        );
        assert_eq!(
            decode_pg_numeric(&numeric_raw(-1, 0x4000, 4, &[12])).unwrap(),
            "-0.0012"
        );
        assert_eq!(
            decode_pg_numeric(&numeric_raw(-2, 0x0000, 8, &[12])).unwrap(),
            "0.00000012"
        );
        assert_eq!(
            decode_pg_numeric(&numeric_raw(-3, 0x4000, 12, &[12])).unwrap(),
            "-0.000000000012"
        );
        assert_eq!(
            decode_pg_numeric(&numeric_raw(1, 0x0000, 0, &[10])).unwrap(),
            "100000"
        );
    }

    #[test]
    fn decodes_postgres_timetz_values() {
        let raw = timetz_raw(45_296_000_000, 18_000);
        assert_eq!(decode_pg_timetz(&raw).unwrap(), "12:34:56-05:00");
    }

    fn numeric_raw(weight: i16, sign: u16, dscale: i16, digits: &[i16]) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(&(digits.len() as i16).to_be_bytes());
        raw.extend_from_slice(&weight.to_be_bytes());
        raw.extend_from_slice(&sign.to_be_bytes());
        raw.extend_from_slice(&dscale.to_be_bytes());
        for digit in digits {
            raw.extend_from_slice(&digit.to_be_bytes());
        }
        raw
    }

    fn timetz_raw(micros: i64, timezone_seconds_west: i32) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(&micros.to_be_bytes());
        raw.extend_from_slice(&timezone_seconds_west.to_be_bytes());
        raw
    }

    fn test_digest(database_id: &str) -> SchemaDigestOutput {
        let tables = vec![SchemaDigestTable {
            schema: "public".to_string(),
            name: "users".to_string(),
            relation_kind: "table".to_string(),
            columns: vec![SchemaDigestColumn {
                name: "id".to_string(),
                data_type: "integer".to_string(),
                nullable: false,
                default_expression: None,
                generated_expression: None,
            }],
            constraints: Vec::new(),
            indexes: vec![SchemaDigestIndex {
                name: "users_pkey".to_string(),
                definition_hash: "original".to_string(),
            }],
            view_definition_hash: None,
        }];
        let extensions = vec![SchemaDigestExtension {
            name: "plpgsql".to_string(),
            version: "1.0".to_string(),
        }];
        SchemaDigestOutput {
            database_id: database_id.to_string(),
            database_name: format!("sandbox_{database_id}"),
            digest_version: SCHEMA_DIGEST_VERSION,
            checksum: "before-checksum".to_string(),
            table_count: 1,
            relation_counts: relation_counts_for_digest_tables(&tables),
            column_count: 1,
            constraint_count: 0,
            index_count: 1,
            extension_count: 1,
            tables,
            extensions,
        }
    }

    fn workflow_test_digest(table_names: Vec<String>) -> WorkflowSchemaDigest {
        let tables = table_names
            .into_iter()
            .map(|name| {
                schema_object_digest(
                    "table",
                    name.clone(),
                    json!({
                        "schema": "public",
                        "name": name,
                        "relationKind": "table",
                        "viewDefinitionHash": null
                    }),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let object_counts = SchemaObjectCounts {
            tables: tables.len(),
            partitioned_tables: 0,
            views: 0,
            materialized_views: 0,
            foreign_tables: 0,
            columns: 0,
            constraints: 0,
            indexes: 0,
            extensions: 0,
        };
        let fingerprint = fingerprint_json(&json!({
            "digestVersion": SCHEMA_DIGEST_VERSION,
            "objectCounts": object_counts.clone(),
            "tables": tables.clone(),
            "columns": [],
            "constraints": [],
            "indexes": [],
            "extensions": []
        }))
        .unwrap();

        WorkflowSchemaDigest {
            digest_version: SCHEMA_DIGEST_VERSION,
            fingerprint,
            object_counts,
            tables,
            columns: Vec::new(),
            constraints: Vec::new(),
            indexes: Vec::new(),
            extensions: Vec::new(),
        }
    }
}
