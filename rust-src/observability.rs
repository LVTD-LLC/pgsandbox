//! Content-free operation envelopes over OTLP/HTTP JSON. Kept beside the
//! existing bounded reqwest capture client: no global logger, SQL instrumentation,
//! panic-message collector, or background exporter thread is installed.
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::telemetry::{EVENT_CLI_INVOCATION_COMPLETED, EVENT_MCP_TOOL_COMPLETED};
use crate::VERSION;

pub struct OperationSignals {
    pub trace_id: String,
    pub logs: Value,
    pub traces: Value,
    pub events: Vec<(&'static str, Map<String, Value>)>,
}

pub fn operation_signals(
    event: &str,
    source: &Map<String, Value>,
    distinct_id: &str,
    end_ns: u128,
) -> Option<OperationSignals> {
    let surface = match event {
        EVENT_CLI_INVOCATION_COMPLETED => "cli",
        EVENT_MCP_TOOL_COMPLETED => "mcp",
        _ => return None, // Legacy detailed CLI events must not double-count.
    };
    let success = source.get("success")?.as_bool()?;
    let elapsed_ms = source.get("elapsedMs")?.as_u64()?;
    // Only enum-like operation names from the existing boundary are accepted.
    // This is a second privacy boundary: never forward arbitrary properties.
    let tool = source
        .get("tool")
        .and_then(Value::as_str)
        .filter(|name| crate::mcp::PUBLIC_MCP_TOOLS.contains(name));
    let command = source
        .get("command")
        .and_then(Value::as_str)
        .filter(|name| {
            matches!(
                *name,
                "setup"
                    | "doctor"
                    | "list-extensions"
                    | "ensure-postgres"
                    | "upgrade"
                    | "uninstall"
                    | "local"
                    | "smoke-test"
                    | "with-database"
                    | "tool"
                    | "unknown"
            )
        });
    let operation = tool.or(command).unwrap_or("unknown");
    let trace_id = Uuid::new_v4().simple().to_string();
    let span_id = Uuid::new_v4().simple().to_string()[..16].to_string();
    let name = format!("pgsandbox.{surface}.{operation}");
    let mut safe = Map::from_iter([
        ("surface".into(), json!(surface)),
        ("operation".into(), json!(operation)),
        ("success".into(), json!(success)),
        ("elapsedMs".into(), json!(elapsed_ms)),
        ("trace_id".into(), json!(trace_id)),
        ("span_id".into(), json!(span_id)),
    ]);
    let mut attributes = vec![
        attr("surface", json!({"stringValue": surface})),
        attr("operation", json!({"stringValue": operation})),
        attr("success", json!({"boolValue": success})),
        attr("elapsedMs", json!({"intValue": elapsed_ms.to_string()})),
        attr("posthogDistinctId", json!({"stringValue": distinct_id})),
    ];
    if source.get("telemetry_test") == Some(&json!(true)) {
        safe.insert("telemetry_test".into(), json!(true));
        attributes.push(attr("telemetry_test", json!({"boolValue": true})));
    }
    let resource = json!({"attributes": [
        attr("service.name", json!({"stringValue": "pgsandbox"})),
        attr("service.version", json!({"stringValue": VERSION})),
        attr("os.type", json!({"stringValue": std::env::consts::OS})),
        attr("host.arch", json!({"stringValue": std::env::consts::ARCH})),
    ]});
    let scope = json!({"name": "pgsandbox.operations", "version": VERSION});
    let logs = json!({"resourceLogs": [{"resource": resource, "scopeLogs": [{
        "scope": scope, "logRecords": [{
            "timeUnixNano": end_ns.to_string(), "observedTimeUnixNano": end_ns.to_string(),
            "severityNumber": if success {9} else {17},
            "severityText": if success {"INFO"} else {"ERROR"},
            "body": {"stringValue": format!("{name} {}", if success {"completed"} else {"failed"})},
            "attributes": attributes, "traceId": trace_id, "spanId": span_id,
            "flags": 1
        }]
    }]}]});
    let traces = json!({"resourceSpans": [{"resource": resource, "scopeSpans": [{
        "scope": scope, "spans": [{
            "traceId": trace_id, "spanId": span_id, "name": name,
            "kind": if surface == "mcp" {2} else {1},
            "startTimeUnixNano": end_ns.saturating_sub(u128::from(elapsed_ms) * 1_000_000).to_string(),
            "endTimeUnixNano": end_ns.to_string(), "attributes": attributes,
            "status": {"code": if success {1} else {2}}, "flags": 1
        }]
    }]}]});
    let mut events = Vec::new();
    if !success {
        let mut error = safe.clone();
        // Intentionally synthetic: no raw error messages or filesystem stack paths.
        error.insert(
            "$exception_list".into(),
            json!([{
                "type": "PGSandboxOperationFailed", "value": format!("{name} failed"),
                "mechanism": {"type": "generic", "handled": true, "synthetic": true}
            }]),
        );
        error.insert("$exception_fingerprint".into(), json!(name));
        error.insert("$exception_level".into(), json!("error"));
        events.push(("$exception", error));
    }
    if surface == "mcp" {
        safe.insert("$ai_trace_id".into(), json!(trace_id));
        safe.insert("$ai_session_id".into(), Value::Null);
        safe.insert("$ai_span_id".into(), json!(span_id));
        safe.insert("$ai_span_name".into(), json!(operation));
        safe.insert("$ai_latency".into(), json!(elapsed_ms as f64 / 1000.0));
        safe.insert("$ai_is_error".into(), json!(!success));
        if !success {
            safe.insert("$ai_error".into(), json!("operation_failed"));
        }
        // Metadata-only tool spans, not fabricated LLM generations or token costs.
        events.push(("$ai_span", safe));
    }
    Some(OperationSignals {
        trace_id,
        logs,
        traces,
        events,
    })
}

fn attr(key: &str, value: Value) -> Value {
    json!({"key": key, "value": value})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_tool_signals_are_correlated_and_content_free() {
        let source = Map::from_iter([
            ("tool".into(), json!("run_sql")),
            ("success".into(), json!(false)),
            ("elapsedMs".into(), json!(25)),
            ("sql".into(), json!("SECRET SQL")),
            ("error".into(), json!("SECRET postgres://user:pass@host/db")),
            ("telemetry_test".into(), json!(true)),
        ]);
        let signals =
            operation_signals(EVENT_MCP_TOOL_COMPLETED, &source, "anonymous", 100_000_000).unwrap();
        let span = &signals.traces["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        let log = &signals.logs["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];
        assert_eq!(span["startTimeUnixNano"], "75000000");
        assert_eq!(span["status"]["code"], 2);
        assert_eq!(log["traceId"], span["traceId"]);
        assert_eq!(log["spanId"], span["spanId"]);
        assert_eq!(signals.events.len(), 2);
        assert_eq!(signals.events[1].1["$ai_trace_id"], span["traceId"]);
        assert_eq!(signals.events[1].1["$ai_latency"], 0.025);
        assert_eq!(signals.events[0].1["surface"], "mcp");
        assert_eq!(signals.events[0].1["telemetry_test"], true);
        let serialized = format!("{}{}{:?}", signals.logs, signals.traces, signals.events);
        assert!(!serialized.contains("SECRET"));
        assert!(!serialized.contains("$ai_generation"));
    }

    #[test]
    fn cli_has_no_ai_events_and_legacy_events_have_no_duplicate_signals() {
        let source = Map::from_iter([
            ("command".into(), json!("SECRET unexpected argv")),
            ("success".into(), json!(true)),
            ("elapsedMs".into(), json!(1)),
        ]);
        let signals =
            operation_signals(EVENT_CLI_INVOCATION_COMPLETED, &source, "anonymous", 0).unwrap();
        assert!(signals.events.is_empty());
        assert!(!signals.logs.to_string().contains("SECRET"));
        assert!(operation_signals(
            crate::telemetry::EVENT_CLI_COMMAND_COMPLETED,
            &source,
            "id",
            0
        )
        .is_none());
        assert!(operation_signals(EVENT_MCP_TOOL_COMPLETED, &Map::new(), "id", 0).is_none());
    }
}
