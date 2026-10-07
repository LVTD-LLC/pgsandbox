use std::{
    fs,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::{config::TelemetryConfig, VERSION};

const POSTHOG_PROJECT_TOKEN: &str = "phc_BGKAJLGN9zQ9BD8LTpRXxsE25BewML4ZnfNR8RtmPQZf";
const POSTHOG_HOST: &str = "https://us.i.posthog.com";
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
    token: String,
    host: String,
    test: bool,
    pending: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl Telemetry {
    pub fn new(config: TelemetryConfig) -> Self {
        Self {
            enabled: config.enabled,
            distinct_id: config.enabled.then(installation_id),
            client: config.enabled.then(reqwest::Client::new),
            token: std::env::var("PGSANDBOX_POSTHOG_KEY")
                .unwrap_or_else(|_| POSTHOG_PROJECT_TOKEN.into()),
            host: std::env::var("PGSANDBOX_POSTHOG_HOST").unwrap_or_else(|_| POSTHOG_HOST.into()),
            test: std::env::var("PGSANDBOX_TELEMETRY_TEST").as_deref() == Ok("1"),
            pending: Arc::default(),
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            distinct_id: None,
            client: None,
            token: String::new(),
            host: String::new(),
            test: false,
            pending: Arc::default(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub async fn capture(&self, event: &'static str, mut properties: Map<String, Value>) {
        if !self.enabled {
            return;
        }
        let Some(distinct_id) = self.distinct_id.as_deref() else {
            return;
        };
        let Some(client) = self.client.as_ref() else {
            return;
        };

        if !self.token.starts_with("phc_") {
            return;
        }
        if self.test {
            properties.insert("telemetry_test".into(), json!(true));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let signals = crate::observability::operation_signals(event, &properties, distinct_id, now);
        if let Some(signals) = &signals {
            properties.insert("trace_id".into(), json!(signals.trace_id));
        }
        let mut batch = vec![capture_payload(distinct_id, event, properties)];
        if let Some(signals) = &signals {
            for (name, props) in &signals.events {
                batch.push(capture_payload(distinct_id, name, props.clone()));
            }
        }
        for item in &mut batch {
            item.as_object_mut().unwrap().remove("api_key");
        }
        let payload = json!({"api_key": self.token, "batch": batch});
        let host = self.host.trim_end_matches('/');
        // All three requests share ONE deadline. Offline observability cannot
        // extend command latency by one timeout per signal or alter the result.
        let send = async {
            let events = client.post(format!("{host}/batch/")).json(&payload).send();
            if let Some(signals) = signals {
                let logs = client
                    .post(format!("{host}/i/v1/logs"))
                    .bearer_auth(&self.token)
                    .json(&signals.logs)
                    .send();
                let traces = client
                    .post(format!("{host}/i/v1/traces"))
                    .bearer_auth(&self.token)
                    .json(&signals.traces)
                    .send();
                let _ = tokio::join!(events, logs, traces);
            } else {
                let _ = events.await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_millis(TELEMETRY_TIMEOUT_MS), send).await;
    }

    pub fn capture_background(&self, event: &'static str, properties: Map<String, Value>) {
        if !self.enabled {
            return;
        }
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|task| !task.is_finished());
            if pending.len() >= 64 {
                return;
            } // Drop telemetry, never backpressure MCP.
            let telemetry = self.clone();
            pending.push(tokio::spawn(async move {
                telemetry.capture(event, properties).await;
            }));
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
    properties.entry("surface".to_string()).or_insert_with(|| {
        json!(if event.starts_with("pgsandbox_cli_") {
            "cli"
        } else {
            "mcp"
        })
    });
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

    // Exercise the actual network boundary, not just payload helper snapshots.
    #[tokio::test]
    async fn operation_sends_three_correlated_requests_and_opt_out_sends_none() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let n = stream.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    buffer.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&buffer[..pos]);
                        let length: usize = header
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap();
                        if buffer.len() >= pos + 4 + length {
                            requests.push((
                                header.lines().next().unwrap().to_owned(),
                                serde_json::from_slice::<Value>(&buffer[pos + 4..pos + 4 + length])
                                    .unwrap(),
                            ));
                            break;
                        }
                    }
                }
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await
                    .unwrap();
            }
            requests
        });
        let mut telemetry = Telemetry::disabled();
        telemetry.enabled = true;
        telemetry.client = Some(reqwest::Client::new());
        telemetry.distinct_id = Some("test-installation".into());
        telemetry.host = host;
        telemetry.token = "phc_test".into();
        telemetry.test = true;
        let props = properties([
            ("tool", json!("run_sql")),
            ("success", json!(false)),
            ("elapsedMs", json!(12)),
        ]);
        telemetry
            .capture(EVENT_MCP_TOOL_COMPLETED, props.clone())
            .await;
        let requests = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
        let batch = &requests
            .iter()
            .find(|(path, _)| path.contains("/batch/"))
            .unwrap()
            .1;
        assert_eq!(batch["api_key"], "phc_test");
        assert_eq!(batch["batch"].as_array().unwrap().len(), 3);
        assert_eq!(batch["batch"][1]["event"], "$exception");
        assert_eq!(batch["batch"][2]["event"], "$ai_span");
        assert_eq!(batch["batch"][0]["properties"]["telemetry_test"], true);
        let trace = &requests
            .iter()
            .find(|(path, _)| path.contains("/traces"))
            .unwrap()
            .1;
        assert_eq!(
            batch["batch"][0]["properties"]["trace_id"],
            trace["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"]
        );
        // A disabled instance must not touch even a listening local receiver.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        telemetry.host = format!("http://{}", listener.local_addr().unwrap());
        telemetry.enabled = false;
        telemetry
            .capture(EVENT_MCP_TOOL_COMPLETED, props.clone())
            .await;
        telemetry.capture_background(EVENT_MCP_TOOL_COMPLETED, props.clone());
        assert!(telemetry.pending.lock().unwrap().is_empty());
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
        // A management token must never be transmitted as an ingestion token.
        telemetry.enabled = true;
        telemetry.token = "phx_never_send".into();
        telemetry.capture(EVENT_MCP_TOOL_COMPLETED, props).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn offline_delivery_has_one_deadline_and_does_not_fail() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut telemetry = Telemetry::disabled();
        telemetry.enabled = true;
        telemetry.client = Some(reqwest::Client::new());
        telemetry.distinct_id = Some("test".into());
        telemetry.host = format!("http://{}", listener.local_addr().unwrap());
        telemetry.token = "phc_test".into();
        let start = std::time::Instant::now();
        telemetry
            .capture(
                EVENT_MCP_TOOL_COMPLETED,
                properties([
                    ("tool", json!("doctor")),
                    ("success", json!(true)),
                    ("elapsedMs", json!(1)),
                ]),
            )
            .await;
        assert!(start.elapsed() < Duration::from_millis(1500));
    }

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
