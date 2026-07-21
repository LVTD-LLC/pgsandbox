use std::time::{SystemTime, UNIX_EPOCH};

use pgsandbox::{
    config::load_config,
    postgres::{
        DatabaseSelector, PostgresSandboxManager, SessionCleanupPolicy, SessionStatus,
        WithDatabaseInput,
    },
};

fn session_input(
    command: &[&str],
    cleanup: SessionCleanupPolicy,
    owner: &str,
) -> WithDatabaseInput {
    WithDatabaseInput {
        repo_path: std::env::current_dir()
            .expect("current directory")
            .display()
            .to_string(),
        profile: None,
        postgres_version: None,
        name_hint: Some("session lifecycle e2e".to_string()),
        ttl_minutes: Some(15),
        owner: Some(owner.to_string()),
        labels: None,
        extensions: None,
        command: command.iter().map(|part| (*part).to_string()).collect(),
        timeout_seconds: Some(10),
        database_url_env_names: None,
        connection_mode: None,
        cleanup,
    }
}

async fn delete_retained(
    manager: &PostgresSandboxManager,
    output: &pgsandbox::postgres::WithDatabaseOutput,
) {
    let sandbox = output.sandbox.as_ref().expect("sandbox identity");
    manager
        .delete_database(DatabaseSelector {
            profile: Some(sandbox.profile.clone()),
            postgres_version: None,
            database_id: Some(sandbox.database_id.clone()),
            database_name: None,
        })
        .await
        .expect("delete retained sandbox");
}

#[tokio::test]
async fn with_database_live_cleanup_matrix() {
    if std::env::var("PGSANDBOX_SESSION_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping session E2E; set PGSANDBOX_SESSION_E2E=1 to run");
        return;
    }

    let manager = PostgresSandboxManager::new(load_config().expect("load PGSandbox config"));
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let owner = format!("pgsandbox-session-e2e-{suffix}");

    let success = manager
        .with_database(
            session_input(
                &[
                    "sh",
                    "-c",
                    "psql \"$DATABASE_URL\" -v ON_ERROR_STOP=1 -Atc 'select 42'",
                ],
                SessionCleanupPolicy::Always,
                &owner,
            ),
            None,
        )
        .await
        .expect("successful session");
    assert_eq!(success.status, SessionStatus::Succeeded);
    assert_eq!(
        success.command.as_ref().and_then(|run| run.exit_code),
        Some(0)
    );
    assert!(success.cleanup.deleted);
    assert!(!success.cleanup.retained);

    let mut failed_input = session_input(
        &["sh", "-c", "exit 17"],
        SessionCleanupPolicy::OnSuccess,
        &owner,
    );
    failed_input.timeout_seconds = Some(2);
    let failed = manager
        .with_database(failed_input, None)
        .await
        .expect("failing child session");
    assert_eq!(failed.status, SessionStatus::ChildFailed);
    assert_eq!(
        failed.command.as_ref().and_then(|run| run.exit_code),
        Some(17)
    );
    assert!(failed.cleanup.retained);
    delete_retained(&manager, &failed).await;

    let kept = manager
        .with_database(
            session_input(
                &["sh", "-c", "test -n \"$DATABASE_URL\""],
                SessionCleanupPolicy::Keep,
                &owner,
            ),
            None,
        )
        .await
        .expect("kept session");
    assert_eq!(kept.status, SessionStatus::Retained);
    assert!(kept.cleanup.retained);
    delete_retained(&manager, &kept).await;

    let mut timeout_input = session_input(
        &["sh", "-c", "sleep 5"],
        SessionCleanupPolicy::Always,
        &owner,
    );
    timeout_input.timeout_seconds = Some(1);
    let timed_out = manager
        .with_database(timeout_input, None)
        .await
        .expect("timed out session");
    assert_eq!(timed_out.status, SessionStatus::TimedOut);
    assert!(timed_out.cleanup.deleted);

    let spawn_failed = manager
        .with_database(
            session_input(
                &["pgsandbox-command-that-does-not-exist"],
                SessionCleanupPolicy::Always,
                &owner,
            ),
            None,
        )
        .await
        .expect("spawn failure session");
    assert_eq!(spawn_failed.status, SessionStatus::ChildSpawnFailed);
    assert!(spawn_failed.cleanup.deleted);
}
