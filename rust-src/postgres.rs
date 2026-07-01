use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
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
    types::{FromSql, ToSql, Type},
    Client, NoTls, Row, SimpleQueryMessage,
};
use url::Url;
use uuid::Uuid;

use crate::{
    config::{find_profile, find_profile_for_request, ConfigError, SandboxConfig, SandboxProfile},
    local::{discover_local_postgres_installations, LocalClusterConfig, LocalPostgresCluster},
    names::{make_sandbox_names, quote_ident, quote_literal},
};

const METADATA_TABLE: &str = "pgsandbox_databases";
const AUDIT_TABLE: &str = "pgsandbox_events";
const DEFAULT_ROW_LIMIT: usize = 100;
const LIST_DATABASES_LIMIT: usize = 100;
const ENCRYPTED_PASSWORD_PREFIX: &str = "v1";
const SCHEMA_DIGEST_VERSION: u32 = 1;
const MAX_SCHEMA_DIFF_ITEMS: usize = 50;
const DEFAULT_WORKFLOW_TIMEOUT_SECONDS: u64 = 120;
const MAX_WORKFLOW_TIMEOUT_SECONDS: u64 = 600;
const MAX_COMMAND_OUTPUT_BYTES: usize = 8_000;
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
    pub owner: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CleanupExpiredInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub dry_run: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDiffInput {
    pub profile: Option<String>,
    pub postgres_version: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
    pub base_digest: Value,
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
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunMigrationsInput {
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
pub struct ValidateMigrationInput {
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
    pub databases: Vec<Value>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListProfilesOutput {
    pub profiles: Vec<ProfileSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub name: String,
    pub default: bool,
    pub postgres_version: Option<String>,
    pub managed_local: bool,
    pub admin_url: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDatabaseOutput {
    pub database_id: String,
    pub profile: String,
    pub database_name: String,
    pub role_name: String,
    pub expires_at: DateTime<Utc>,
    pub connection_string: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneDatabaseOutput {
    pub database_id: String,
    pub profile: String,
    pub database_name: String,
    pub role_name: String,
    pub expires_at: DateTime<Utc>,
    pub connection_string: String,
    pub source: String,
    pub schema_only: bool,
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
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunSqlOutput {
    pub database_id: String,
    pub database_name: String,
    pub row_count: Option<u64>,
    pub rows: Vec<Value>,
    pub truncated: bool,
    pub elapsed_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DescribeSchemaOutput {
    pub database_id: String,
    pub database_name: String,
    pub tables: Vec<Value>,
    pub columns: Vec<Value>,
    pub indexes: Vec<Value>,
    pub extensions: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupExpiredOutput {
    pub dry_run: bool,
    pub selected: Option<Vec<Value>>,
    pub deleted: Option<Vec<String>>,
    pub failures: Option<Vec<Value>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestOutput {
    pub database_id: String,
    pub database_name: String,
    pub digest_version: u32,
    pub checksum: String,
    pub table_count: usize,
    pub column_count: usize,
    pub index_count: usize,
    pub extension_count: usize,
    pub tables: Vec<SchemaDigestTable>,
    pub extensions: Vec<SchemaDigestExtension>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestTable {
    pub schema: String,
    pub name: String,
    pub columns: Vec<SchemaDigestColumn>,
    pub indexes: Vec<SchemaDigestIndex>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestIndex {
    pub name: String,
    pub definition_hash: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDigestExtension {
    pub name: String,
    pub version: String,
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
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowError {
    pub code: String,
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
    pub indexes: Vec<SchemaObjectDigest>,
    pub extensions: Vec<SchemaObjectDigest>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaObjectCounts {
    pub tables: usize,
    pub columns: usize,
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
    pub detected_framework: Option<String>,
    pub postgres_version: Option<String>,
    pub postgres_version_source: Option<String>,
    pub config_path: Option<String>,
    pub sandbox_target: Option<String>,
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
pub struct ValidateMigrationOutput {
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSandboxFromTemplateOutput {
    pub database_id: String,
    pub profile: String,
    pub database_name: String,
    pub role_name: String,
    pub expires_at: DateTime<Utc>,
    pub connection_string: String,
    pub template_name: String,
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
    rows: Vec<Value>,
    truncated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoProjectConfig {
    framework: String,
    migration_command: Vec<String>,
    seed_command: Option<Vec<String>>,
    database_url_env: String,
    #[serde(default)]
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
            Ok(profile) => Ok(profile.clone()),
            Err(ConfigError::UnknownPostgresVersion(version))
                if self.config.managed_local.enabled && profile_name.is_none() =>
            {
                self.ensure_managed_local_profile(Some(&version))
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
            .context("failed to prepare requested local Postgres version")?;
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
                managed_local: profile.managed_local,
                admin_url: mask_connection_string(&profile.admin_url),
                source: "configured".to_string(),
            })
            .collect::<Vec<_>>();

        if self.config.managed_local.enabled && input.include_discovered_local.unwrap_or(true) {
            for installation in discover_local_postgres_installations() {
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
                    postgres_version: Some(installation.postgres_version),
                    managed_local: true,
                    admin_url: "(managed local; starts on demand)".to_string(),
                    source: installation.source,
                });
            }
        }

        Ok(ListProfilesOutput { profiles })
    }

    async fn get_owned_record(
        &self,
        profile_name: Option<String>,
        postgres_version: Option<String>,
        database_id: Option<String>,
        database_name: Option<String>,
    ) -> anyhow::Result<(SandboxProfile, SandboxRecord)> {
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

        Ok(CreateDatabaseOutput {
            database_id: names.database_id,
            profile: profile.name.clone(),
            database_name: names.database_name.clone(),
            role_name: names.role_name.clone(),
            expires_at,
            connection_string: build_connection_string(
                &profile.admin_url,
                &names.database_name,
                &names.role_name,
                &role_password,
            )?,
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
        let created = self
            .create_database(CreateDatabaseInput {
                profile,
                postgres_version,
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
            connection_string: created.connection_string,
            source: "external".to_string(),
            schema_only,
        })
    }

    pub async fn delete_database(
        &self,
        input: DatabaseSelector,
    ) -> anyhow::Result<DeleteDatabaseOutput> {
        let profile =
            self.resolve_profile(input.profile.as_deref(), input.postgres_version.as_deref())?;
        let (client, connection_task) = connect_admin(&profile).await?;
        ensure_metadata_table(&client).await?;
        let record = find_record(&client, &profile.name, &input)
            .await?
            .context("Database was not found in PGSandbox metadata.")?;

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
        let profile =
            self.resolve_profile(input.profile.as_deref(), input.postgres_version.as_deref())?;
        let (client, connection_task) = connect_admin(&profile).await?;
        ensure_metadata_table(&client).await?;
        let record = find_record(&client, &profile.name, &input)
            .await?
            .context("Database was not found in PGSandbox metadata.")?;
        drop(client);
        let _ = connection_task.await;

        Ok(ConnectionStringOutput {
            database_id: record.database_id,
            database_name: record.database_name.clone(),
            expires_at: record.expires_at,
            connection_string: build_connection_string(
                &profile.admin_url,
                &record.database_name,
                &record.role_name,
                &unprotect_role_password(&record.role_password, &profile)?,
            )?,
        })
    }

    pub async fn list_databases(
        &self,
        input: ListDatabasesInput,
    ) -> anyhow::Result<ListDatabasesOutput> {
        let profile =
            self.resolve_profile(input.profile.as_deref(), input.postgres_version.as_deref())?;
        let (client, connection_task) = connect_admin(&profile).await?;
        ensure_metadata_table(&client).await?;
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
                &[&profile.name, &input.owner],
            )
            .await?;
        drop(client);
        let _ = connection_task.await;

        let truncated = rows.len() > LIST_DATABASES_LIMIT;
        Ok(ListDatabasesOutput {
            databases: rows
                .iter()
                .take(LIST_DATABASES_LIMIT)
                .map(record_summary_to_json)
                .collect(),
            truncated,
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
                  SELECT table_schema, table_name
                  FROM information_schema.tables
                  WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
                  ORDER BY table_schema, table_name
                "#,
                &[],
            )
            .await?;
        let columns = client
            .query(
                r#"
                  SELECT table_schema, table_name, column_name, data_type, is_nullable
                  FROM information_schema.columns
                  WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
                  ORDER BY table_schema, table_name, ordinal_position
                "#,
                &[],
            )
            .await?;
        let indexes = client
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
        let extensions = client
            .query(
                "SELECT extname, extversion FROM pg_extension ORDER BY extname",
                &[],
            )
            .await?;

        drop(client);
        let _ = connection_task.await;

        Ok(DescribeSchemaOutput {
            database_id: connection.database_id,
            database_name: connection.database_name,
            tables: rows_to_json(tables)?,
            columns: rows_to_json(columns)?,
            indexes: rows_to_json(indexes)?,
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
        let before = serde_json::from_value::<SchemaDigestOutput>(input.base_digest)
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
        let (client, connection_task) = connect_url(&connection_string).await?;
        let postgres_version = postgres_version(&client).await?;
        let digest = collect_schema_digest(&client).await?;
        drop(client);
        let _ = connection_task.await;

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
        write_json_file(&paths.metadata_path, &snapshot)?;
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
        let current = collect_schema_digest_for_url(&connection_string).await?;
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

        let postgres_version =
            resolve_repo_postgres_version(&repo_path, input.postgres_version.clone())?;
        let detection = detect_django_repo(&repo_path)?;
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

        if !detection {
            return Ok(workflow_failure(
                "Repository detection needs an explicit adapter.",
                workflow_error(
                    "repo_detection_uncertain",
                    "Could not confidently detect a Django repo from manage.py and settings patterns.",
                    Some("Pass explicit commands to run_migrations/seed_database or add Django project files.".to_string()),
                ),
                Some(PrepareForRepoOutput {
                    repo_path: repo_path.display().to_string(),
                    detected_framework: None,
                    postgres_version: postgres_version.version.clone(),
                    postgres_version_source: postgres_version.source.clone(),
                    config_path: None,
                    sandbox_target,
                    action_needed: Some("Add manage.py plus a Django settings module, or provide explicit commands to the workflow tools.".to_string()),
                }),
            ));
        }

        let project_config = RepoProjectConfig {
            framework: "django".to_string(),
            migration_command: default_django_migration_command(),
            seed_command: None,
            database_url_env: "DATABASE_URL".to_string(),
            postgres_version: postgres_version.version.clone(),
            prepared_at: Utc::now(),
        };
        let config_path = write_repo_project_config(&repo_path, &project_config)?;
        let output = PrepareForRepoOutput {
            repo_path: repo_path.display().to_string(),
            detected_framework: Some("django".to_string()),
            postgres_version: postgres_version.version,
            postgres_version_source: postgres_version.source,
            config_path: Some(config_path.display().to_string()),
            sandbox_target,
            action_needed: None,
        };

        Ok(workflow_success(
            "Django repository prepared for PG Sandbox workflows.",
            None,
            Vec::new(),
            vec![json!({
                "type": "repo-config",
                "framework": "django"
            })],
            output,
        ))
    }

    pub async fn run_migrations(
        &self,
        input: RunMigrationsInput,
    ) -> anyhow::Result<WorkflowEnvelope<CommandWorkflowOutput>> {
        if !selector_has_database(&input.database_id, &input.database_name) {
            return Ok(workflow_failure(
                "Migrations were not run.",
                workflow_error(
                    "missing_sandbox",
                    "run_migrations requires databaseId or databaseName.",
                    Some("Create a sandbox first, then pass its databaseId.".to_string()),
                ),
                None,
            ));
        }
        let repo_path = PathBuf::from(&input.repo_path);
        if !repo_path.is_dir() {
            return Ok(workflow_failure(
                "Migrations were not run.",
                repo_not_found_error(&repo_path),
                None,
            ));
        }
        let command = match resolve_migration_command(&repo_path, input.command)? {
            Ok(command) => command,
            Err(error) => return Ok(workflow_failure("Migrations were not run.", error, None)),
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
            run_repo_command(&repo_path, &command, &connection.connection_string, timeout).await?;
        let output = command_workflow_output(
            &connection.database_id,
            &connection.database_name,
            command_result,
        );
        let ok = output.exit_code == Some(0);

        Ok(if ok {
            workflow_success(
                "Migrations completed successfully.",
                None,
                Vec::new(),
                vec![json!({
                    "type": "migration-run",
                    "databaseId": connection.database_id
                })],
                output,
            )
        } else {
            workflow_failure(
                "Migrations failed.",
                workflow_error(
                    "migration_failed",
                    format!("Migration command exited with {:?}", output.exit_code),
                    Some(
                        "Inspect stderr/stdout in the result and rerun after fixing the migration."
                            .to_string(),
                    ),
                ),
                Some(output),
            )
        })
    }

    pub async fn validate_migration(
        &self,
        input: ValidateMigrationInput,
    ) -> anyhow::Result<WorkflowEnvelope<ValidateMigrationOutput>> {
        let repo_path = PathBuf::from(&input.repo_path);
        if !repo_path.is_dir() {
            return Ok(workflow_failure(
                "Migration validation was not run.",
                repo_not_found_error(&repo_path),
                None,
            ));
        }
        let command = match resolve_migration_command(&repo_path, input.command)? {
            Ok(command) => command,
            Err(error) => {
                return Ok(workflow_failure(
                    "Migration validation was not run.",
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
                            .unwrap_or_else(|| "django migration validation".to_string()),
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
        let validation_result = async {
            let before = collect_schema_digest_for_url(&connection.connection_string).await?;
            let command_result =
                run_repo_command(&repo_path, &command, &connection.connection_string, timeout)
                    .await?;
            let after = collect_schema_digest_for_url(&connection.connection_string).await?;
            let diff = diff_workflow_schema_digests(&before, &after);
            anyhow::Ok((command_result, diff))
        }
        .await;
        let (command_result, diff) = match validation_result {
            Ok(result) => result,
            Err(error) if created_sandbox => {
                let cleanup = self
                    .delete_database(DatabaseSelector {
                        profile: created_profile.clone(),
                        postgres_version: None,
                        database_id: Some(connection.database_id.clone()),
                        database_name: None,
                    })
                    .await;
                return match cleanup {
                    Ok(_) => Ok(workflow_failure(
                        "Migration validation failed before completion; the created sandbox was deleted.",
                        workflow_error(
                            "migration_validation_error",
                            error.to_string(),
                            Some("Retry after fixing the validation error. No sandbox cleanup is required.".to_string()),
                        ),
                        None,
                    )),
                    Err(cleanup_error) => Err(anyhow::anyhow!(
                        "migration validation failed and cleanup also failed for {}: {error}; cleanup error: {cleanup_error}",
                        connection.database_name
                    )),
                };
            }
            Err(error) => return Err(error),
        };
        let ok = command_result.exit_code == Some(0);
        let output = validate_migration_output(
            &connection.database_id,
            &connection.database_name,
            created_sandbox,
            command_result,
            diff.clone(),
        );

        if ok {
            return Ok(workflow_success(
                "Migration validation completed successfully.",
                Some(diff.changed_objects),
                Vec::new(),
                vec![json!({
                    "type": "migration-validation",
                    "databaseId": connection.database_id,
                    "createdSandbox": created_sandbox
                })],
                output,
            ));
        }

        let deleted_auto_sandbox = if created_sandbox {
            let cleanup = self
                .delete_database(DatabaseSelector {
                    profile: created_profile,
                    postgres_version: None,
                    database_id: Some(connection.database_id.clone()),
                    database_name: None,
                })
                .await;
            match cleanup {
                Ok(_) => true,
                Err(cleanup_error) => {
                    return Err(anyhow::anyhow!(
                        "migration validation failed and cleanup also failed for {}: cleanup error: {cleanup_error}",
                        connection.database_name
                    ))
                }
            }
        } else {
            false
        };

        Ok(workflow_failure_with_changes(
            if deleted_auto_sandbox {
                "Migration validation failed; the created sandbox was deleted."
            } else {
                "Migration validation failed."
            },
            diff.changed_objects,
            workflow_error(
                "migration_failed",
                format!("Migration command exited with {:?}", output.exit_code),
                Some(
                    if deleted_auto_sandbox {
                        "Inspect stderr/stdout and rerun after fixing the migration. No sandbox cleanup is required."
                    } else {
                        "Inspect stderr/stdout in the result and the schema diff before retrying."
                    }
                    .to_string(),
                ),
            ),
            Some(output),
        ))
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
            run_repo_command(&repo_path, &command, &connection.connection_string, timeout).await?;
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

        Ok(workflow_success(
            format!(
                "Sandbox created from template `{}`.",
                metadata.template_name
            ),
            None,
            vec![metadata.privacy_warning.clone()],
            vec![template_detail_handle(&profile.name, &template_name)],
            CreateSandboxFromTemplateOutput {
                database_id: created.database_id,
                profile: created.profile,
                database_name: created.database_name,
                role_name: created.role_name,
                expires_at: created.expires_at,
                connection_string: created.connection_string,
                template_name,
            },
        ))
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
        let profile =
            self.resolve_profile(input.profile.as_deref(), input.postgres_version.as_deref())?;
        let (client, connection_task) = connect_admin(&profile).await?;
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

        if input.dry_run.unwrap_or(false) {
            drop(client);
            let _ = connection_task.await;
            return Ok(CleanupExpiredOutput {
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
            dry_run: false,
            selected: None,
            deleted: Some(deleted),
            failures: Some(failures),
        })
    }
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
    }
}

fn workflow_error(
    code: impl Into<String>,
    message: impl Into<String>,
    hint: Option<String>,
) -> WorkflowError {
    WorkflowError {
        code: code.into(),
        message: message.into(),
        hint,
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

async fn collect_schema_digest_for_url(database_url: &str) -> anyhow::Result<WorkflowSchemaDigest> {
    let (client, connection_task) = connect_url(database_url).await?;
    let digest = collect_schema_digest(&client).await?;
    drop(client);
    let _ = connection_task.await;
    Ok(digest)
}

async fn collect_schema_digest(client: &Client) -> anyhow::Result<WorkflowSchemaDigest> {
    let table_rows = client
        .query(
            r#"
              SELECT table_schema, table_name, table_type
              FROM information_schema.tables
              WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
              ORDER BY table_schema, table_name
            "#,
            &[],
        )
        .await?;
    let column_rows = client
        .query(
            r#"
              SELECT table_schema, table_name, column_name, ordinal_position,
                     data_type, udt_name, is_nullable, column_default
              FROM information_schema.columns
              WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
              ORDER BY table_schema, table_name, ordinal_position
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
        let table_type: String = row.get("table_type");
        tables.push(schema_object_digest(
            "table",
            format!("{schema}.{name}"),
            json!({
                "schema": schema,
                "name": name,
                "tableType": table_type
            }),
        )?);
    }

    let mut columns = Vec::new();
    for row in column_rows {
        let schema: String = row.get("table_schema");
        let table: String = row.get("table_name");
        let name: String = row.get("column_name");
        let ordinal_position: i32 = row.get("ordinal_position");
        let data_type: String = row.get("data_type");
        let udt_name: String = row.get("udt_name");
        let is_nullable: String = row.get("is_nullable");
        let column_default: Option<String> = row.get("column_default");
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
                "columnDefault": column_default
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

    let object_counts = SchemaObjectCounts {
        tables: tables.len(),
        columns: columns.len(),
        indexes: indexes.len(),
        extensions: extensions.len(),
    };
    let canonical = json!({
        "digestVersion": SCHEMA_DIGEST_VERSION,
        "objectCounts": object_counts.clone(),
        "tables": tables.clone(),
        "columns": columns.clone(),
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

fn schema_object_map(digest: &WorkflowSchemaDigest) -> BTreeMap<String, SchemaObjectDigest> {
    digest
        .tables
        .iter()
        .chain(digest.columns.iter())
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

fn default_django_migration_command() -> Vec<String> {
    vec![
        "python".to_string(),
        "manage.py".to_string(),
        "migrate".to_string(),
        "--noinput".to_string(),
    ]
}

fn detect_django_repo(repo_path: &Path) -> anyhow::Result<bool> {
    let manage_py = repo_path.join("manage.py");
    if !manage_py.is_file() {
        return Ok(false);
    }
    let manage_text = fs::read_to_string(&manage_py).unwrap_or_default();
    if manage_text.contains("DJANGO_SETTINGS_MODULE") {
        return Ok(true);
    }
    for entry in fs::read_dir(repo_path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && path.join("settings.py").is_file() {
            return Ok(true);
        }
    }
    Ok(false)
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
            let value = serde_yaml::from_str::<serde_yaml::Value>(&raw)
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
    value: &serde_yaml::Value,
    file_name: &str,
    path: Vec<String>,
) -> Option<RepoPostgresVersionInference> {
    match value {
        serde_yaml::Value::Mapping(mapping) => {
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
        serde_yaml::Value::Sequence(items) => {
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
    if !(image.contains("postgres") || image.contains("postgis") || image.contains("timescaledb")) {
        return None;
    }
    let tag = image.rsplit_once(':')?.1;
    leading_digits(tag).or_else(|| {
        tag.split(['-', '_', '.'])
            .find_map(|part| part.strip_prefix("pg").and_then(leading_digits))
    })
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

fn resolve_migration_command(
    repo_path: &Path,
    input_command: Option<Vec<String>>,
) -> anyhow::Result<Result<Vec<String>, WorkflowError>> {
    let command = match input_command {
        Some(command) => command,
        None => match read_repo_project_config(repo_path)? {
            Some(config) => config.migration_command,
            None => {
                return Ok(Err(workflow_error(
                    "missing_migration_command",
                    "No migration command was provided and .pgsandbox/project.json is missing.",
                    Some(
                        "Run prepare_for_repo or pass an explicit Django migrate command."
                            .to_string(),
                    ),
                )))
            }
        },
    };
    if !command_is_bounded(&command) {
        return Ok(Err(workflow_error(
            "unclear_command",
            "Migration command is empty or too large.",
            Some("Pass a short argv array such as [\"python\", \"manage.py\", \"migrate\", \"--noinput\"].".to_string()),
        )));
    }
    if !is_known_django_migration_command(&command) {
        return Ok(Err(workflow_error(
            "unsafe_migration_command",
            "Only explicit Django migrate commands are allowed for run_migrations and validate_migration.",
            Some("Use python manage.py migrate --noinput, or configure that command in .pgsandbox/project.json.".to_string()),
        )));
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
    if !command_is_bounded(&command) {
        return Ok(Err(workflow_error(
            "unclear_command",
            "Seed command is empty or too large.",
            Some("Pass a short argv array. Commands are executed without a shell.".to_string()),
        )));
    }
    Ok(Ok(command))
}

fn command_is_bounded(command: &[String]) -> bool {
    if command.is_empty() || command.len() > 16 {
        return false;
    }
    let total_len = command.iter().map(String::len).sum::<usize>();
    total_len <= 2_048
        && command.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 256
                && !part.contains('\0')
                && !part.contains('\n')
                && !part.contains('\r')
        })
}

fn is_known_django_migration_command(command: &[String]) -> bool {
    if command.len() < 3 {
        return false;
    }
    let (manage_index, migrate_index) = if command[0] == "manage.py" || command[0] == "./manage.py"
    {
        (0, 1)
    } else if is_python_command(&command[0])
        && command.get(1).map(String::as_str) == Some("manage.py")
    {
        (1, 2)
    } else {
        return false;
    };
    if command.get(manage_index).map(String::as_str) != Some("manage.py")
        && command.get(manage_index).map(String::as_str) != Some("./manage.py")
    {
        return false;
    }
    if command.get(migrate_index).map(String::as_str) != Some("migrate") {
        return false;
    }
    command[migrate_index + 1..].iter().all(|arg| {
        !arg.contains(';')
            && !arg.contains('&')
            && !arg.contains('|')
            && !arg.contains('>')
            && !arg.contains('<')
            && !arg.starts_with("--database")
    })
}

fn is_python_command(command: &str) -> bool {
    let executable = Path::new(command)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(command);
    executable == "python"
        || executable == "python3"
        || executable
            .strip_prefix("python3.")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
}

fn workflow_timeout(timeout_seconds: Option<u64>) -> StdDuration {
    StdDuration::from_secs(
        timeout_seconds
            .unwrap_or(DEFAULT_WORKFLOW_TIMEOUT_SECONDS)
            .min(MAX_WORKFLOW_TIMEOUT_SECONDS),
    )
}

async fn run_repo_command(
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
    let mut child = command_builder
        .args(&command[1..])
        .current_dir(repo_path)
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
            let _ = child.kill().await;
            let _ = child.wait().await;
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

fn validate_migration_output(
    database_id: &str,
    database_name: &str,
    created_sandbox: bool,
    result: CommandRunResult,
    schema_diff: WorkflowSchemaDiffOutput,
) -> ValidateMigrationOutput {
    ValidateMigrationOutput {
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
    connect_url(&profile.admin_url).await
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
              SELECT n.nspname AS table_schema, c.relname AS table_name
              FROM pg_class c
              JOIN pg_namespace n ON n.oid = c.relnamespace
              WHERE c.relkind IN ('r', 'p')
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
                     NOT a.attnotnull AS nullable
              FROM pg_attribute a
              JOIN pg_class c ON c.oid = a.attrelid
              JOIN pg_namespace n ON n.oid = c.relnamespace
              WHERE c.relkind IN ('r', 'p')
                AND a.attnum > 0
                AND NOT a.attisdropped
                AND n.nspname NOT IN ('pg_catalog', 'information_schema')
              ORDER BY n.nspname, c.relname, a.attnum
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
        tables.insert(
            (schema.clone(), name.clone()),
            SchemaDigestTable {
                schema,
                name,
                columns: Vec::new(),
                indexes: Vec::new(),
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
                columns: Vec::new(),
                indexes: Vec::new(),
            });
        table.columns.push(SchemaDigestColumn {
            name: row.get("column_name"),
            data_type: row.get("data_type"),
            nullable: row.get("nullable"),
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
    let extensions = extension_rows
        .into_iter()
        .map(|row| SchemaDigestExtension {
            name: row.get("extname"),
            version: row.get("extversion"),
        })
        .collect::<Vec<_>>();

    let table_count = tables.len();
    let column_count = tables.iter().map(|table| table.columns.len()).sum();
    let index_count = tables.iter().map(|table| table.indexes.len()).sum();
    let extension_count = extensions.len();
    let checksum = schema_digest_checksum(&tables, &extensions)?;

    Ok(SchemaDigestOutput {
        database_id,
        database_name,
        digest_version: 1,
        checksum,
        table_count,
        column_count,
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
        digest_version: 1,
        tables,
        extensions,
    };
    Ok(sha256_hex(&serde_json::to_vec(&content)?))
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
        (Err(error), _) => Err(error),
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
    Ok(QueryExecutionResult {
        row_count: if truncated {
            None
        } else {
            Some(visible_rows.len() as u64)
        },
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
    Ok(QueryExecutionResult {
        row_count: if truncated {
            None
        } else {
            Some(visible_rows.len() as u64)
        },
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
    QueryExecutionResult {
        row_count: final_row_count,
        rows: final_rows.into_iter().take(row_limit).collect(),
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
    json!({
        "database_id": row.get::<_, String>("database_id"),
        "profile_name": row.get::<_, String>("profile_name"),
        "database_name": row.get::<_, String>("database_name"),
        "role_name": row.get::<_, String>("role_name"),
        "owner": row.get::<_, Option<String>>("owner"),
        "purpose": row.get::<_, Option<String>>("purpose"),
        "labels": row.get::<_, Value>("labels"),
        "created_at": row.get::<_, DateTime<Utc>>("created_at"),
        "expires_at": row.get::<_, DateTime<Utc>>("expires_at"),
        "deleted_at": row.get::<_, Option<DateTime<Utc>>>("deleted_at"),
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
            columns: vec![SchemaDigestColumn {
                name: "id".to_string(),
                data_type: "integer".to_string(),
                nullable: false,
            }],
            indexes: vec![SchemaDigestIndex {
                name: "users_pkey".to_string(),
                definition_hash: "abc123".to_string(),
            }],
        }];
        let extensions = vec![SchemaDigestExtension {
            name: "plpgsql".to_string(),
            version: "1.0".to_string(),
        }];

        let first = SchemaDigestOutput {
            database_id: "db_a".to_string(),
            database_name: "sandbox_a".to_string(),
            digest_version: 1,
            checksum: schema_digest_checksum(&tables, &extensions).unwrap(),
            table_count: 1,
            column_count: 1,
            index_count: 1,
            extension_count: 1,
            tables: tables.clone(),
            extensions: extensions.clone(),
        };
        let second = SchemaDigestOutput {
            database_id: "db_b".to_string(),
            database_name: "sandbox_b".to_string(),
            digest_version: 1,
            checksum: schema_digest_checksum(&tables, &extensions).unwrap(),
            table_count: 1,
            column_count: 1,
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
        });
        after.tables[0].indexes[0].definition_hash = "changed".to_string();
        after.tables.push(SchemaDigestTable {
            schema: "public".to_string(),
            name: "posts".to_string(),
            columns: vec![SchemaDigestColumn {
                name: "id".to_string(),
                data_type: "integer".to_string(),
                nullable: false,
            }],
            indexes: Vec::new(),
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
    fn schema_diff_rejects_mismatched_digest_versions() {
        let before = test_digest("before");
        let mut after = test_digest("after");
        after.digest_version = 2;

        let error = diff_schema_digests(&before, &after).unwrap_err();

        assert!(error.to_string().contains("schema digest versions differ"));
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
            validate_artifact_name("django_seed.v1", "templateName").unwrap(),
            "django_seed.v1"
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
    fn detects_django_repo_and_writes_secret_free_project_config() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        std::fs::write(
            repo.join("manage.py"),
            "import os\nos.environ.setdefault('DJANGO_SETTINGS_MODULE', 'app.settings')\n",
        )
        .unwrap();

        assert!(detect_django_repo(repo).unwrap());

        let config = RepoProjectConfig {
            framework: "django".to_string(),
            migration_command: default_django_migration_command(),
            seed_command: None,
            database_url_env: "DATABASE_URL".to_string(),
            postgres_version: None,
            prepared_at: Utc::now(),
        };
        let path = write_repo_project_config(repo, &config).unwrap();
        let raw = std::fs::read_to_string(path).unwrap();

        assert!(raw.contains("\"framework\": \"django\""));
        assert!(raw.contains("\"manage.py\""));
        assert!(!raw.contains("postgres://"));
        assert!(!raw.contains("secret"));
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
            framework: "django".to_string(),
            migration_command: default_django_migration_command(),
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
    fn migration_commands_are_limited_to_django_migrate() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();

        assert!(resolve_migration_command(
            repo,
            Some(vec![
                "python".to_string(),
                "manage.py".to_string(),
                "migrate".to_string(),
                "--noinput".to_string(),
            ]),
        )
        .unwrap()
        .is_ok());

        let shell = resolve_migration_command(
            repo,
            Some(vec![
                "bash".to_string(),
                "-lc".to_string(),
                "python manage.py migrate".to_string(),
            ]),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(shell.code, "unsafe_migration_command");

        let shell_management_command = resolve_migration_command(
            repo,
            Some(vec![
                "python".to_string(),
                "manage.py".to_string(),
                "shell".to_string(),
            ]),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(shell_management_command.code, "unsafe_migration_command");

        let fake_python = resolve_migration_command(
            repo,
            Some(vec![
                "python-malicious".to_string(),
                "manage.py".to_string(),
                "migrate".to_string(),
            ]),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(fake_python.code, "unsafe_migration_command");

        let alternate_database = resolve_migration_command(
            repo,
            Some(vec![
                "python".to_string(),
                "manage.py".to_string(),
                "migrate".to_string(),
                "--database=default".to_string(),
            ]),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(alternate_database.code, "unsafe_migration_command");

        let custom_settings = resolve_migration_command(
            repo,
            Some(vec![
                "python".to_string(),
                "manage.py".to_string(),
                "migrate".to_string(),
                "--settings=myapp.settings.ci".to_string(),
                "--pythonpath".to_string(),
                "src".to_string(),
            ]),
        )
        .unwrap();
        assert!(custom_settings.is_ok());
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
                "python".to_string(),
                "manage.py".to_string(),
                "loaddata".to_string(),
                "fixture.json".to_string(),
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
            columns: vec![SchemaDigestColumn {
                name: "id".to_string(),
                data_type: "integer".to_string(),
                nullable: false,
            }],
            indexes: vec![SchemaDigestIndex {
                name: "users_pkey".to_string(),
                definition_hash: "original".to_string(),
            }],
        }];
        let extensions = vec![SchemaDigestExtension {
            name: "plpgsql".to_string(),
            version: "1.0".to_string(),
        }];
        SchemaDigestOutput {
            database_id: database_id.to_string(),
            database_name: format!("sandbox_{database_id}"),
            digest_version: 1,
            checksum: "before-checksum".to_string(),
            table_count: 1,
            column_count: 1,
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
                        "tableType": "BASE TABLE"
                    }),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let object_counts = SchemaObjectCounts {
            tables: tables.len(),
            columns: 0,
            indexes: 0,
            extensions: 0,
        };
        let fingerprint = fingerprint_json(&json!({
            "digestVersion": SCHEMA_DIGEST_VERSION,
            "objectCounts": object_counts.clone(),
            "tables": tables.clone(),
            "columns": [],
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
            indexes: Vec::new(),
            extensions: Vec::new(),
        }
    }
}
