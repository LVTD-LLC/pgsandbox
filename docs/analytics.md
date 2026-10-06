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
