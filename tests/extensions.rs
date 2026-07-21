use std::time::{SystemTime, UNIX_EPOCH};

use pgsandbox::{
    config::load_config,
    postgres::{
        CreateDatabaseInput, DatabaseSelector, ListExtensionsInput, PostgresSandboxManager,
        RunSqlInput,
    },
};
use serde_json::{json, Value};

#[tokio::test]
async fn create_database_installs_requested_extensions_when_enabled() {
    if std::env::var("PGSANDBOX_EXTENSION_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping extension E2E; set PGSANDBOX_EXTENSION_E2E=1 to run");
        return;
    }

    let extension =
        std::env::var("PGSANDBOX_EXTENSION_E2E_NAME").unwrap_or_else(|_| "pg_trgm".to_string());
    let expected_extension = extension.trim().to_ascii_lowercase();
    let manager = PostgresSandboxManager::new(load_config().expect("load PGSandbox config"));
    let owner = format!("pgsandbox-extension-{}", unique_suffix());
    let profile_extensions = manager
        .list_extensions(ListExtensionsInput {
            profile: None,
            postgres_version: None,
            database_id: None,
            database_name: None,
        })
        .await
        .expect("list available extensions");
    if !profile_extensions
        .available_extensions
        .iter()
        .any(|available| available.name == expected_extension)
    {
        panic!("expected available extension {expected_extension} in selected profile");
    }

    let created = manager
        .create_database(CreateDatabaseInput {
            profile: None,
            postgres_version: None,
            name_hint: Some("extension e2e".to_string()),
            ttl_minutes: Some(30),
            owner: Some(owner),
            labels: Some([("suite".to_string(), json!("extensions"))].into()),
            extensions: Some(vec![extension]),
        })
        .await
        .expect("create extension sandbox");

    let result = async {
        let expected_extensions = std::slice::from_ref(&expected_extension);
        if created.installed_extensions.as_slice() != expected_extensions {
            anyhow::bail!(
                "expected installedExtensions {:?}, got {:?}",
                expected_extensions,
                created.installed_extensions
            );
        }
        let sandbox_extensions = manager
            .list_extensions(ListExtensionsInput {
                profile: Some(created.profile.clone()),
                postgres_version: None,
                database_id: Some(created.database_id.clone()),
                database_name: None,
            })
            .await?;
        if !sandbox_extensions
            .installed_extensions
            .iter()
            .any(|installed| installed.name == expected_extension)
        {
            anyhow::bail!(
                "expected installed extension {expected_extension} in {:?}",
                sandbox_extensions.installed_extensions
            );
        }

        let lookup = manager
            .run_sql(RunSqlInput {
                profile: Some(created.profile.clone()),
                postgres_version: None,
                database_id: Some(created.database_id.clone()),
                database_name: None,
                sql: "SELECT extname FROM pg_extension ORDER BY extname".to_string(),
                readonly: Some(true),
                row_limit: None,
            })
            .await?;
        let installed = lookup
            .rows
            .iter()
            .filter_map(|row| row.get("extname").and_then(Value::as_str))
            .collect::<Vec<_>>();

        if !installed.contains(&expected_extension.as_str()) {
            anyhow::bail!("expected extension {expected_extension} in {installed:?}");
        }

        let privileges = manager
            .run_sql(RunSqlInput {
                profile: Some(created.profile.clone()),
                postgres_version: None,
                database_id: Some(created.database_id.clone()),
                database_name: None,
                sql: "SELECT rolsuper, rolcreatedb FROM pg_roles WHERE rolname = current_user"
                    .to_string(),
                readonly: Some(true),
                row_limit: None,
            })
            .await?;
        let role = privileges
            .rows
            .first()
            .ok_or_else(|| anyhow::anyhow!("sandbox role privilege row was missing"))?;
        if role.get("rolsuper") != Some(&json!(false))
            || role.get("rolcreatedb") != Some(&json!(false))
        {
            anyhow::bail!("sandbox role gained elevated privileges: {role:?}");
        }
        Ok::<(), anyhow::Error>(())
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
        eprintln!("extension E2E cleanup failed: {error:#}");
    }

    result.expect("extension E2E");
}

fn unique_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}
