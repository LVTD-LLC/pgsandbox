use std::{
    fs,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::{config::TelemetryConfig, VERSION};

const POSTHOG_PROJECT_TOKEN: &str = "phc_BGKAJLGN9zQ9BD8LTpRXxsE25BewML4ZnfNR8RtmPQZf";
const POSTHOG_CAPTURE_URL: &str = "https://us.i.posthog.com/i/v0/e/";
const TELEMETRY_TIMEOUT_MS: u64 = 750;

pub const EVENT_CLI_INVOCATION_COMPLETED: &str = "pgsandbox_cli_invocation_completed";

pub const EVENT_CLI_COMMAND_COMPLETED: &str = "pgsandbox_cli_command_completed";
pub const EVENT_MCP_TOOL_COMPLETED: &str = "pgsandbox_tool_completed";
pub const EVENT_MCP_SERVER_STARTED: &str = "pgsandbox_server_started";

static SESSION_INSTALLATION_ID: LazyLock<String> = LazyLock::new(|| Uuid::new_v4().to_string());

#[derive(Clone)]
pub struct Telemetry {
    enabled: bool,
    distinct_id: Option<String>,
    client: Option<reqwest::Client>,
    pending: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl Telemetry {
    pub fn new(config: TelemetryConfig) -> Self {
        Self {
            enabled: config.enabled,
            distinct_id: config.enabled.then(installation_id),
            client: config.enabled.then(reqwest::Client::new),
            pending: Arc::default(),
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            distinct_id: None,
            client: None,
            pending: Arc::default(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub async fn capture(&self, event: &'static str, properties: Map<String, Value>) {
        if !self.enabled {
            return;
        }
        let Some(distinct_id) = self.distinct_id.as_deref() else {
            return;
        };
        let Some(client) = self.client.as_ref() else {
            return;
        };

        let payload = capture_payload(distinct_id, event, properties);
        let request = client.post(POSTHOG_CAPTURE_URL).json(&payload).send();
        let _ = tokio::time::timeout(Duration::from_millis(TELEMETRY_TIMEOUT_MS), request).await;
    }

    pub fn capture_background(&self, event: &'static str, properties: Map<String, Value>) {
        if !self.enabled {
            return;
        }
        let telemetry = self.clone();
        let handle = tokio::spawn(async move {
            telemetry.capture(event, properties).await;
        });
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|task| !task.is_finished());
            pending.push(handle);
        }
    }

    /// Give in-flight MCP events a bounded opportunity to finish before runtime shutdown.
    pub async fn flush(&self) {
        let tasks = self
            .pending
            .lock()
            .map(|mut tasks| std::mem::take(&mut *tasks))
            .unwrap_or_default();
        let _ = tokio::time::timeout(Duration::from_millis(TELEMETRY_TIMEOUT_MS + 100), async {
            for task in tasks {
                let _ = task.await;
            }
        })
        .await;
    }
}

pub fn properties(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn capture_payload(distinct_id: &str, event: &str, mut properties: Map<String, Value>) -> Value {
    properties.insert(
        "surface".to_string(),
        json!(if event.starts_with("pgsandbox_cli_") {
            "cli"
        } else {
            "mcp"
        }),
    );
    properties.insert("telemetrySchemaVersion".to_string(), json!(2));
    properties.insert("$geoip_disable".to_string(), json!(true));
    properties.insert("app".to_string(), json!("pgsandbox"));
    properties.insert("version".to_string(), json!(VERSION));
    properties.insert("os".to_string(), json!(std::env::consts::OS));
    properties.insert("arch".to_string(), json!(std::env::consts::ARCH));
    properties.insert("$process_person_profile".to_string(), json!(false));

    json!({
        "api_key": POSTHOG_PROJECT_TOKEN,
        "event": event,
        "distinct_id": distinct_id,
        "properties": properties
    })
}

fn installation_id() -> String {
    let Some(mut path) = dirs::config_dir() else {
        return session_installation_id();
    };
    path.push("pgsandbox");
    path.push("telemetry-id");

    if let Ok(existing) = fs::read_to_string(&path) {
        let existing = existing.trim();
        if Uuid::parse_str(existing).is_ok() {
            return existing.to_string();
        }
    }

    let id = Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_ok() && fs::write(&path, &id).is_ok() {
            return id;
        }
    }

    session_installation_id()
}

fn session_installation_id() -> String {
    SESSION_INSTALLATION_ID.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn flush_waits_for_pending_tasks() {
        let telemetry = Telemetry::disabled();
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = completed.clone();
        telemetry
            .pending
            .lock()
            .unwrap()
            .push(tokio::spawn(async move {
                tokio::task::yield_now().await;
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }));
        telemetry.flush().await;
        assert!(completed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(telemetry.pending.lock().unwrap().is_empty());
    }

    #[test]
    fn payload_marks_events_as_personless() {
        let payload = capture_payload(
            "install-id",
            EVENT_MCP_TOOL_COMPLETED,
            properties([("tool", json!("create_database"))]),
        );

        assert_eq!(payload["api_key"], POSTHOG_PROJECT_TOKEN);
        assert_eq!(payload["event"], EVENT_MCP_TOOL_COMPLETED);
        assert_eq!(payload["distinct_id"], "install-id");
        assert_eq!(payload["properties"]["tool"], "create_database");
        assert_eq!(payload["properties"]["app"], "pgsandbox");
        assert_eq!(payload["properties"]["surface"], "mcp");
        assert_eq!(payload["properties"]["$process_person_profile"], false);
    }

    #[test]
    fn disabled_telemetry_has_no_distinct_id() {
        let telemetry = Telemetry::disabled();

        assert!(!telemetry.is_enabled());
        assert!(telemetry.distinct_id.is_none());
        assert!(telemetry.client.is_none());
    }
}
