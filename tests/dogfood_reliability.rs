use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use pgsandbox_mcp::{
    config::load_config,
    postgres::{
        CloneDatabaseInput, CreateDatabaseInput, CreateSandboxFromTemplateInput,
        CreateSchemaSnapshotInput, CreateTemplateFromSandboxInput, DatabaseSelector,
        DeleteTemplateInput, DescribeSchemaInput, DiffSchemaSnapshotInput, ExplainQueryInput,
        ListDatabasesInput, ListSchemaSnapshotsInput, ListTemplatesInput, PostgresSandboxManager,
        RunSqlInput, SchemaDiffInput, ValidateSchemaChangeInput,
    },
};
use serde_json::json;

#[tokio::test]
async fn dogfood_reliability_suite_runs_when_enabled() {
    if std::env::var("PGSANDBOX_DOGFOOD_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping dogfood E2E; set PGSANDBOX_DOGFOOD_E2E=1 to run");
        return;
    }

    let manager = PostgresSandboxManager::new(load_config().expect("load PGSandbox config"));
    let owner = format!("pgsandbox-dogfood-{}", unique_suffix());
    let created = manager
        .create_database(CreateDatabaseInput {
            profile: None,
            postgres_version: None,
            name_hint: Some("dogfood reliability".to_string()),
            ttl_minutes: Some(45),
            owner: Some(owner.clone()),
            labels: Some([("suite".to_string(), json!("dogfood"))].into()),
        })
        .await
        .expect("create dogfood sandbox");

    let result = run_suite(
        &manager,
        &created.database_id,
        &created.profile,
        &created.connection_string,
        &owner,
    )
    .await;

    let cleanup = manager
        .delete_database(DatabaseSelector {
            profile: Some(created.profile),
            postgres_version: None,
            database_id: Some(created.database_id),
            database_name: None,
        })
        .await;
    if let Err(error) = cleanup {
        eprintln!("dogfood cleanup failed: {error:#}");
    }

    result.expect("dogfood reliability suite");
}

#[tokio::test]
async fn pg18_schema_snapshot_minimal_schema_returns_without_timeout_when_enabled() {
    if std::env::var("PGSANDBOX_DOGFOOD_PG18_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping PG18 schema snapshot E2E; set PGSANDBOX_DOGFOOD_PG18_E2E=1 to run");
        return;
    }

    let manager = PostgresSandboxManager::new(load_config().expect("load PGSandbox config"));
    let owner = format!("pgsandbox-pg18-snapshot-{}", unique_suffix());
    let created = manager
        .create_database(CreateDatabaseInput {
            profile: None,
            postgres_version: Some("18".to_string()),
            name_hint: Some("pg18 snapshot regression".to_string()),
            ttl_minutes: Some(30),
            owner: Some(owner),
            labels: Some([("suite".to_string(), json!("pg18-schema-snapshot"))].into()),
        })
        .await
        .expect("create PG18 snapshot regression sandbox");

    let result = async {
        let selector = || DatabaseSelector {
            profile: Some(created.profile.clone()),
            postgres_version: None,
            database_id: Some(created.database_id.clone()),
            database_name: None,
        };

        manager
            .run_sql(RunSqlInput {
                profile: selector().profile,
                postgres_version: None,
                database_id: selector().database_id,
                database_name: None,
                sql: "\
                    CREATE EXTENSION IF NOT EXISTS pgcrypto;\
                    CREATE TABLE accounts(\
                        id uuid PRIMARY KEY DEFAULT gen_random_uuid(),\
                        email text NOT NULL UNIQUE\
                    );\
                    CREATE TABLE events(\
                        id bigserial PRIMARY KEY,\
                        account_id uuid NOT NULL REFERENCES accounts(id),\
                        name text NOT NULL,\
                        created_at timestamptz NOT NULL DEFAULT now()\
                    );\
                "
                .to_string(),
                readonly: None,
                row_limit: None,
            })
            .await?;

        let snapshot = tokio::time::timeout(
            StdDuration::from_secs(10),
            manager.create_schema_snapshot(CreateSchemaSnapshotInput {
                profile: selector().profile,
                postgres_version: None,
                database_id: selector().database_id,
                database_name: None,
                snapshot_name: "baseline_e2e_041".to_string(),
                notes: Some("PGSBX-041 regression".to_string()),
            }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("create_schema_snapshot timed out on a tiny PG18 schema"))??;
        anyhow::ensure!(snapshot.ok, "snapshot was not ok: {snapshot:?}");

        let snapshots = manager
            .list_schema_snapshots(ListSchemaSnapshotsInput {
                profile: selector().profile,
                postgres_version: None,
                database_id: selector().database_id,
                database_name: None,
            })
            .await?;
        let snapshot_summaries = snapshots
            .result
            .ok_or_else(|| anyhow::anyhow!("list_schema_snapshots returned no result"))?;
        anyhow::ensure!(
            snapshot_summaries.len() == 1,
            "expected 1 snapshot, got {}",
            snapshot_summaries.len()
        );

        anyhow::Ok(())
    }
    .await;

    let cleanup = manager
        .delete_database(DatabaseSelector {
            profile: Some(created.profile),
            postgres_version: None,
            database_id: Some(created.database_id),
            database_name: None,
        })
        .await;
    if let Err(error) = cleanup {
        eprintln!("PG18 snapshot regression cleanup failed: {error:#}");
    }

    result.expect("PG18 schema snapshot regression");
}

async fn run_suite(
    manager: &PostgresSandboxManager,
    database_id: &str,
    profile: &str,
    connection_string: &str,
    owner: &str,
) -> anyhow::Result<()> {
    let seeded = manager
        .run_sql(RunSqlInput {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
            sql: "CREATE TABLE accounts(id serial PRIMARY KEY, email text UNIQUE); INSERT INTO accounts(email) VALUES ('a@example.com'), ('b@example.com'), ('c@example.com');".to_string(),
            readonly: None,
            row_limit: None,
        })
        .await?;
    assert_eq!(seeded.returned_row_count, 0);
    assert_eq!(seeded.affected_row_count, Some(3));

    let limited = manager
        .run_sql(RunSqlInput {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
            sql: "SELECT * FROM accounts ORDER BY id".to_string(),
            readonly: Some(true),
            row_limit: Some(2),
        })
        .await?;
    assert_eq!(limited.returned_row_count, 2);
    assert!(limited.truncated);

    let before = manager
        .schema_digest(DatabaseSelector {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
        })
        .await?;

    let described = manager
        .describe_schema(DescribeSchemaInput {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
        })
        .await?;
    assert!(described.relation_counts.tables >= 1);

    let plan = manager
        .explain_query(ExplainQueryInput {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
            sql: "SELECT * FROM accounts WHERE id = 1".to_string(),
        })
        .await?;
    assert!(plan.summary.is_object());

    let snapshot = manager
        .create_schema_snapshot(CreateSchemaSnapshotInput {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
            snapshot_name: "before_repo_change".to_string(),
            notes: Some("dogfood baseline".to_string()),
        })
        .await?;
    assert!(snapshot.ok, "{snapshot:?}");

    let snapshots = manager
        .list_schema_snapshots(ListSchemaSnapshotsInput {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
        })
        .await?;
    assert_eq!(snapshots.result.unwrap().len(), 1);

    manager
        .run_sql(RunSqlInput {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
            sql: "ALTER TABLE accounts ADD COLUMN status text NOT NULL DEFAULT 'active'; CREATE INDEX accounts_status_idx ON accounts(status);".to_string(),
            readonly: None,
            row_limit: None,
        })
        .await?;

    let diff = manager
        .schema_diff(SchemaDiffInput {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
            base_digest: before,
        })
        .await?;
    assert!(diff.changed);

    let snapshot_diff = manager
        .diff_schema_snapshot(DiffSchemaSnapshotInput {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
            snapshot_name: "before_repo_change".to_string(),
        })
        .await?;
    assert!(snapshot_diff.ok);

    exercise_templates(manager, database_id, owner).await?;
    exercise_clone(manager, profile, connection_string, owner).await?;

    validate_repo_command(manager, owner).await?;

    let remaining = manager
        .list_databases(ListDatabasesInput {
            profile: None,
            postgres_version: Some("*".to_string()),
            include_all_versions: Some(true),
            owner: Some(owner.to_string()),
        })
        .await?;
    assert_eq!(remaining.databases.len(), 1, "{:?}", remaining.databases);

    Ok(())
}

async fn exercise_templates(
    manager: &PostgresSandboxManager,
    database_id: &str,
    owner: &str,
) -> anyhow::Result<()> {
    let template_name = format!("dogfood_{}", unique_suffix());
    let created = manager
        .create_template_from_sandbox(CreateTemplateFromSandboxInput {
            profile: None,
            postgres_version: None,
            database_id: Some(database_id.to_string()),
            database_name: None,
            template_name: template_name.clone(),
            created_by: Some(owner.to_string()),
            notes: Some("dogfood template".to_string()),
        })
        .await?;
    assert!(created.ok, "{created:?}");

    let templates = manager
        .list_templates(ListTemplatesInput {
            profile: None,
            postgres_version: None,
        })
        .await?;
    assert!(templates
        .result
        .unwrap_or_default()
        .iter()
        .any(|template| template.template_name == template_name));

    let restored = manager
        .create_sandbox_from_template(CreateSandboxFromTemplateInput {
            profile: None,
            postgres_version: None,
            template_name: template_name.clone(),
            name_hint: Some("dogfood restore".to_string()),
            ttl_minutes: Some(45),
            owner: Some(owner.to_string()),
            labels: Some([("suite".to_string(), json!("dogfood"))].into()),
        })
        .await?;
    assert!(restored.ok, "{restored:?}");
    let restored_id = restored
        .result
        .as_ref()
        .expect("restored sandbox")
        .database_id
        .clone();

    manager
        .delete_database(DatabaseSelector {
            profile: None,
            postgres_version: None,
            database_id: Some(restored_id),
            database_name: None,
        })
        .await?;

    let deleted = manager
        .delete_template(DeleteTemplateInput {
            profile: None,
            postgres_version: None,
            template_name,
        })
        .await?;
    assert!(deleted.result.unwrap().deleted);

    Ok(())
}

async fn exercise_clone(
    manager: &PostgresSandboxManager,
    profile: &str,
    connection_string: &str,
    owner: &str,
) -> anyhow::Result<()> {
    let cloned = manager
        .clone_database(CloneDatabaseInput {
            profile: Some(profile.to_string()),
            postgres_version: None,
            source_database_url: connection_string.to_string(),
            name_hint: Some("dogfood clone".to_string()),
            ttl_minutes: Some(45),
            owner: Some(owner.to_string()),
            labels: Some([("suite".to_string(), json!("dogfood"))].into()),
            schema_only: Some(true),
        })
        .await?;

    manager
        .delete_database(DatabaseSelector {
            profile: Some(cloned.profile),
            postgres_version: None,
            database_id: Some(cloned.database_id),
            database_name: None,
        })
        .await?;

    Ok(())
}

async fn validate_repo_command(
    manager: &PostgresSandboxManager,
    owner: &str,
) -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;

    let validation = manager
        .validate_schema_change(ValidateSchemaChangeInput {
            repo_path: directory.path().display().to_string(),
            profile: None,
            postgres_version: None,
            database_id: None,
            database_name: None,
            command: Some(vec![
                "psql".to_string(),
                "-v".to_string(),
                "ON_ERROR_STOP=1".to_string(),
                "-c".to_string(),
                "CREATE TABLE repo_items(id serial PRIMARY KEY); CREATE INDEX repo_items_id_idx ON repo_items(id);"
                    .to_string(),
            ]),
            timeout_seconds: Some(20),
            name_hint: Some("dogfood validate".to_string()),
            ttl_minutes: Some(45),
            owner: Some(owner.to_string()),
            labels: Some([("suite".to_string(), json!("dogfood"))].into()),
        })
        .await?;
    assert!(validation.ok, "{validation:?}");
    let created = validation.result.expect("validation output");
    assert!(created.created_sandbox);
    assert!(created.schema_diff.changed_objects.added > 0);

    manager
        .delete_database(DatabaseSelector {
            profile: None,
            postgres_version: None,
            database_id: Some(created.database_id),
            database_name: None,
        })
        .await?;

    Ok(())
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_millis()
}
