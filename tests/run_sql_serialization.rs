use std::time::{SystemTime, UNIX_EPOCH};

use pgsandbox_mcp::{
    config::load_config,
    postgres::{CreateDatabaseInput, DatabaseSelector, PostgresSandboxManager, RunSqlInput},
};
use serde_json::{json, Value};

#[tokio::test]
async fn run_sql_preserves_to_regclass_nullability_with_cast_hint_when_enabled() {
    if !run_sql_serialization_e2e_enabled() {
        return;
    }

    let manager = PostgresSandboxManager::new(load_config().expect("load PGSandbox config"));
    let owner = format!("pgsandbox-run-sql-{}", unique_suffix());
    let created = manager
        .create_database(CreateDatabaseInput {
            profile: None,
            postgres_version: None,
            name_hint: Some("run sql serialization".to_string()),
            ttl_minutes: Some(30),
            owner: Some(owner),
            labels: Some([("suite".to_string(), json!("run_sql_serialization"))].into()),
        })
        .await
        .expect("create serialization sandbox");

    let result = async {
        manager
            .run_sql(RunSqlInput {
                profile: None,
                postgres_version: None,
                database_id: Some(created.database_id.clone()),
                database_name: None,
                sql: "CREATE TABLE validation_table(id integer PRIMARY KEY);".to_string(),
                readonly: None,
                row_limit: None,
            })
            .await?;

        let lookup = manager
            .run_sql(RunSqlInput {
                profile: None,
                postgres_version: None,
                database_id: Some(created.database_id.clone()),
                database_name: None,
                sql: "SELECT to_regclass('public.validation_table') AS validation_table, to_regclass('public.missing_validation_table') AS missing_table".to_string(),
                readonly: Some(true),
                row_limit: None,
            })
            .await?;

        let row = lookup.rows.first().expect("to_regclass returned one row");
        let existing = row
            .get("validation_table")
            .expect("validation_table column is present");
        assert_eq!(
            existing.get("unsupportedPostgresType").and_then(Value::as_str),
            Some("regclass"),
            "expected regclass to preserve its original type, got {existing:?}"
        );
        assert!(
            existing
                .get("hint")
                .and_then(Value::as_str)
                .is_some_and(|hint| hint.contains("::text")),
            "expected regclass unsupported-type object to include a cast hint, got {existing:?}"
        );
        assert_eq!(row.get("missing_table"), Some(&Value::Null));

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
        eprintln!("run_sql serialization cleanup failed: {error:#}");
    }

    result.expect("run_sql serialization E2E");
}

#[tokio::test]
async fn run_sql_returns_typed_ordered_result_sets_for_multi_statement_sql_when_enabled() {
    if !run_sql_serialization_e2e_enabled() {
        return;
    }

    let manager = PostgresSandboxManager::new(load_config().expect("load PGSandbox config"));
    let owner = format!("pgsandbox-run-sql-{}", unique_suffix());
    let created = manager
        .create_database(CreateDatabaseInput {
            profile: None,
            postgres_version: None,
            name_hint: Some("run sql result sets".to_string()),
            ttl_minutes: Some(30),
            owner: Some(owner),
            labels: Some([("suite".to_string(), json!("run_sql_serialization"))].into()),
        })
        .await
        .expect("create serialization sandbox");

    let result = async {
        let ddl_dml_select = manager
            .run_sql(RunSqlInput {
                profile: None,
                postgres_version: None,
                database_id: Some(created.database_id.clone()),
                database_name: None,
                sql: r#"
                    CREATE TABLE type_check(id integer PRIMARY KEY, payload jsonb NOT NULL);
                    INSERT INTO type_check(id, payload)
                    VALUES (1, '{"name":"alpha"}'::jsonb), (2, '{"name":"beta"}'::jsonb);
                    SELECT count(*)::int AS row_count,
                           jsonb_agg(payload ORDER BY id) AS payloads
                    FROM type_check;
                "#
                .to_string(),
                readonly: None,
                row_limit: None,
            })
            .await?;

        assert_typed_final_type_check_row(&serde_json::to_value(&ddl_dml_select)?);

        let multiple_selects = manager
            .run_sql(RunSqlInput {
                profile: None,
                postgres_version: None,
                database_id: Some(created.database_id.clone()),
                database_name: None,
                sql: r#"
                    SELECT 1::int AS ignored;
                    SELECT count(*)::int AS row_count,
                           jsonb_agg(payload ORDER BY id) AS payloads
                    FROM type_check;
                "#
                .to_string(),
                readonly: Some(true),
                row_limit: None,
            })
            .await?;

        let value = serde_json::to_value(&multiple_selects)?;
        assert_typed_final_type_check_row(&value);
        assert_eq!(
            value
                .get("resultSets")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2),
            "expected both SELECT result sets to be exposed, got {value:#}"
        );
        assert_eq!(
            value["resultSets"][0]["rows"][0]["ignored"],
            json!(1),
            "expected the first SELECT to remain typed and ordered, got {value:#}"
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
        eprintln!("run_sql result-set cleanup failed: {error:#}");
    }

    result.expect("run_sql multi-statement result-set E2E");
}

fn run_sql_serialization_e2e_enabled() -> bool {
    if std::env::var("PGSANDBOX_RUN_SQL_SERIALIZATION_E2E")
        .ok()
        .as_deref()
        == Some("1")
    {
        return true;
    }

    eprintln!(
        "skipping run_sql serialization E2E; set PGSANDBOX_RUN_SQL_SERIALIZATION_E2E=1 to run"
    );
    false
}

fn assert_typed_final_type_check_row(value: &Value) {
    let result_sets = value
        .get("resultSets")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected explicit resultSets array, got {value:#}"));
    let final_result = result_sets
        .last()
        .unwrap_or_else(|| panic!("expected at least one result set, got {value:#}"));
    let final_row = final_result["rows"]
        .as_array()
        .and_then(|rows| rows.first())
        .unwrap_or_else(|| panic!("expected final result set row, got {value:#}"));

    assert_eq!(final_row["row_count"], json!(2));
    assert_eq!(
        final_row["payloads"],
        json!([
            { "name": "alpha" },
            { "name": "beta" }
        ])
    );
    assert_eq!(
        value["rows"][0], *final_row,
        "top-level rows should mirror the final row-returning result set"
    );
}

fn unique_suffix() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_millis();
    format!("{now}_{}", std::process::id())
}
