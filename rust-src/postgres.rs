use std::{collections::BTreeMap, sync::LazyLock};

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
use tokio_postgres::{
    types::{FromSql, ToSql, Type},
    Client, NoTls, Row, SimpleQueryMessage,
};
use url::Url;
use uuid::Uuid;

use crate::{
    config::{find_profile, SandboxConfig, SandboxProfile},
    names::{make_sandbox_names, quote_ident, quote_literal},
};

const METADATA_TABLE: &str = "pgsandbox_databases";
const DEFAULT_ROW_LIMIT: usize = 100;
const LIST_DATABASES_LIMIT: usize = 100;
const ENCRYPTED_PASSWORD_PREFIX: &str = "v1";

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
    pub name_hint: Option<String>,
    pub ttl_minutes: Option<u32>,
    pub owner: Option<String>,
    pub labels: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseSelector {
    pub profile: Option<String>,
    pub database_id: Option<String>,
    pub database_name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunSqlInput {
    pub profile: Option<String>,
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
    pub owner: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CleanupExpiredInput {
    pub profile: Option<String>,
    pub dry_run: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDatabasesOutput {
    pub databases: Vec<Value>,
    pub truncated: bool,
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

    pub async fn create_database(
        &self,
        input: CreateDatabaseInput,
    ) -> anyhow::Result<CreateDatabaseOutput> {
        let profile = find_profile(&self.config, input.profile.as_deref())?.clone();
        let ttl_minutes = clamp_ttl(input.ttl_minutes, &profile)?;
        let names = make_sandbox_names(&profile.database_prefix, input.name_hint.as_deref());
        let role_password = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let expires_at = Utc::now() + Duration::minutes(ttl_minutes.into());

        let (client, connection_task) = connect_admin(&profile).await?;
        ensure_metadata_table(&client).await?;

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

    pub async fn delete_database(
        &self,
        input: DatabaseSelector,
    ) -> anyhow::Result<DeleteDatabaseOutput> {
        let profile = find_profile(&self.config, input.profile.as_deref())?.clone();
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
        let profile = find_profile(&self.config, input.profile.as_deref())?.clone();
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
        let profile = find_profile(&self.config, input.profile.as_deref())?.clone();
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

    pub async fn cleanup_expired(
        &self,
        input: CleanupExpiredInput,
    ) -> anyhow::Result<CleanupExpiredOutput> {
        let profile = find_profile(&self.config, input.profile.as_deref())?.clone();
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
                Ok(()) => deleted.push(record.database_id),
                Err(error) => failures.push(json!({
                    "databaseId": record.database_id,
                    "message": error.to_string()
                })),
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
    Ok(())
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
}
