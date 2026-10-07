# Analytics coverage

All surfaces use PostHog US project **471530 (PG Sandbox)**. Website deployment
reads the public ingestion token from the `PUBLIC_POSTHOG_KEY` repository variable.
For local site builds, use `site/.env` (see `site/.env.example`). A personal
management API key must never be used in a browser or release binary.

## Event contract

- Website: `$pageview`, `$pageleave`, `$autocapture`, web vitals, browser errors,
  dead clicks and heatmaps through the official JS SDK. Custom events:
  `pgsandbox_setup_prompt_copied`, `pgsandbox_setup_prompt_copy_failed`,
  `pgsandbox_docs_link_clicked`, `pgsandbox_outbound_link_clicked`.
  Setup copy success fires only after clipboard success, not on click intent.
- CLI: `pgsandbox_cli_invocation_completed` is the comprehensive invocation
  counter. It includes allowlisted command/tool names, success, exit code,
  elapsed milliseconds, help/dry-run flags, and allowlisted local subcommands.
  Failures during argument/config parsing are included. Top-level help/version
  and MCP serving are excluded. Existing `pgsandbox_cli_command_completed`
  remains a separate, detailed legacy event for selected commands: **do not sum
  these two event types to count invocations**.
- MCP: `pgsandbox_server_started` and `pgsandbox_tool_completed`. Tool events
  include tool, success, duration and existing bounded feature-selection
  properties. Business envelopes with `ok: false` are recorded as failures.
  Pending sends get at most 850ms to finish at graceful server shutdown.
  Forced termination, invalid protocol requests before handler dispatch,
  offline operation and opted-out users are not guaranteed to produce events.

All new events carry `surface` (`website`, `cli`, `mcp`), `app`, and
`telemetrySchemaVersion: 2`. Runtime events also carry version, OS and architecture.
Older releases do not have these new properties or invocation events. Website
merges deploy automatically; runtime instrumentation reaches installed users
only after a new binary release and upgrade.

## Privacy and identity

Runtime identity is an anonymous per-installation UUID; browser identity is
PostHog's anonymous browser ID. No login exists, so these cannot establish a
person-level website-to-install funnel. Do not equate server starts with users.
Runtime opt-outs documented in README remain unchanged, and runtime IP geolocation
is disabled. No SQL, argv values, paths, credentials, database names, query results
or raw runtime errors are added to telemetry.

Website respects Do Not Track and only initializes on the production hostnames.
Autocaptured text/attributes and replay text/inputs are masked; recording console
logs are disabled. Replay activation is controlled by the PostHog project setting.
URL property query strings and fragments are stripped before sending; standard
SDK campaign dimensions remain available separately. Browser errors are intended
for the public static site, not a SQL editor or application containing secrets.

## Verification and monitoring

Use the event contract above to query counts grouped by event and surface. Use
`pgsandbox_cli_invocation_completed` for new CLI usage, tool completion for MCP
usage, and pageviews on the production hostname for website traffic. Inspect
`success`, `elapsedMs`, version, OS and architecture for reliability/adoption.
Synthetic browser checks use `utm_source=analytics_qa`; exclude that source from
traffic reports. Development/preview hostnames do not send website events.

Audit baseline, 2026-10-06 (previous 30 days): 1,305 server starts, 25 MCP tool
completions, one legacy CLI completion, one anonymous installation ID, zero
pageviews. This proves ingestion for that installation, not broader adoption.

## Runtime observability (2026-10-07)

The existing project is reused, not duplicated. Each comprehensive CLI invocation
and MCP tool completion emits an OTLP JSON operation log and a root operation span.
`trace_id` on the product event correlates to the log and trace. Legacy detailed
CLI events do not generate a second log/trace. Logs preserve existing stderr/stdout
behavior; no arbitrary console, process, PostgreSQL, or SQL logs are exported.

Failures emit `$exception` with a synthetic, handled `PGSandboxOperationFailed`
exception grouped by surface/operation. The message is generated from the
allowlisted operation name, never from the original error. This gives operation
failure counts, **not** production stack traces or panic/crash coverage. Inspect
local diagnostics for the original error. Browser exceptions retain SDK stacks;
the deploy build uploads private source maps and removes them before packaging.

MCP completions additionally emit `$ai_span` with operation name, elapsed seconds,
success and a shared trace ID. These are independent root tool spans: the MCP
caller does not supply a parent model trace, and we do not pretend these capture
an entire agent conversation. No `$ai_generation`, prompts, SQL, responses, model,
token counts or costs are invented. Instrument the calling agent to measure those.

All runtime signals share the existing opt-out configuration and one 750ms
network deadline per capture (parallel requests, no retries). MCP sends run in
the background and graceful shutdown drains pending work for at most 850ms.
Offline, opted-out, forcibly terminated, and older installations remain invisible.

Optional runtime overrides (read once at initialization):

- `PGSANDBOX_POSTHOG_KEY`: public `phc_` ingestion token for a different project;
  defaults to the existing public distribution token. Personal tokens are rejected.
- `PGSANDBOX_POSTHOG_HOST`: ingestion origin; defaults to `https://us.i.posthog.com`.
  Use the matching region for your project, or a local HTTP receiver for tests.
- `PGSANDBOX_TELEMETRY_TEST=1`: marks every signal `telemetry_test: true`.
  Exclude this in usage dashboards; it does not bypass telemetry opt-outs.

Website builds retain `PUBLIC_POSTHOG_KEY`. Production deployments additionally
require GitHub secret `POSTHOG_SOURCEMAP_TOKEN` (management token with error-tracking
write permission). It is consumed only by the build plugin, never bundled in the
site. Local/PR builds without that secret skip uploads.

References: [OTLP traces](https://posthog.com/docs/distributed-tracing/start-here),
[OTLP logs](https://posthog.com/docs/logs/installation/other),
[manual exceptions](https://posthog.com/docs/error-tracking/installation/manual),
[AI spans](https://posthog.com/docs/ai-observability/installation/manual-capture),
[Vite source maps](https://posthog.com/docs/error-tracking/upload-source-maps/vite).
