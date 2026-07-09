---
title: "Postgres MCP Server Error Handling for Coding Agents"
excerpt: "Handle Postgres MCP server errors by branching on stable error codes, SQLSTATE, categories, hints, and diagnostic handles instead of retrying every failure blindly."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-09"
updatedAt: "2026-07-09"
tags: ["Postgres", "MCP", "error handling", "AI agents", "SQL"]
category: "Engineering"
metaTitle: "Postgres MCP Server Error Handling for Agents"
metaDescription: "Handle Postgres MCP server errors with stable codes, SQLSTATE, categories, hints, diagnostics, and safe retry rules for coding agents."
canonicalUrl: "https://pgsandbox.lvtd.dev/blog/postgres-mcp-server-error-handling-coding-agents/"
heroImageUrl: ""
featured: false
sortOrder: 120
---
Handle Postgres MCP server errors by reading the structured error envelope first: `errors[].code`, `errors[].category`, optional `errors[].sqlstate`, `errors[].hint`, and any diagnostic `detailHandles`. A coding agent should branch on those fields, not on free-form prose, stack traces, or repeated blind retries.

For PGSandbox MCP, that means a failed tool call is still useful output. The agent gets a compact recovery contract: what failed, which class of failure it belongs to, whether Postgres supplied SQLSTATE, and what the next safe action is.

The short runbook is:

1. Parse the MCP tool response envelope.
2. If `ok` is `false`, read the first structured error.
3. Branch on `error.code`.
4. Use `error.category` for broad routing such as SQL repair, validation, timeout, version selection, or connection diagnostics.
5. Preserve `sqlstate` when Postgres supplies it.
6. Follow `hint` before retrying.
7. Use `detailHandles` for safe follow-up tools such as `list_profiles`, `doctor`, or `describe_schema`.
8. Do not echo raw database URLs, credentials, or full source connection strings into the agent transcript.

The information-gain point is the remediation envelope. A generic Postgres MCP server can return "tool failed" and leave the agent to guess. PGSandbox turns common database failures into reviewable control flow for agents working in [disposable Postgres sandboxes](https://pgsandbox.lvtd.dev/blog/what-is-database-sandbox/).

## Why Postgres MCP errors need structure

Postgres already has a mature error system. PostgreSQL's error-code appendix says server messages are assigned five-character SQLSTATE codes and that applications should usually test the code rather than the localized text (https://www.postgresql.org/docs/current/errcodes-appendix.html). That is exactly the right habit for agents.

MCP adds another layer. The Model Context Protocol tools specification says servers must validate tool inputs, implement access controls, rate-limit tool calls, and sanitize tool outputs; clients should show tool inputs, validate results, implement timeouts, and log usage for audit (https://modelcontextprotocol.io/specification/2025-06-18/server/tools). Error handling is part of that safety boundary, not a cosmetic response format.

For a coding agent, an error is useful only if it changes the next action. These two responses should not be treated the same:

```json
{
  "ok": false,
  "errors": [
    {
      "code": "undefined_column",
      "category": "sql_analysis",
      "sqlstate": "42703",
      "hint": "Call describe_schema or check identifier spelling/casing before retrying."
    }
  ]
}
```

```json
{
  "ok": false,
  "errors": [
    {
      "code": "postgres_auth_failed",
      "category": "postgres",
      "hint": "Run pgsandbox doctor to identify the active config source."
    }
  ]
}
```

The first is a query repair problem. The agent can inspect schema and revise SQL. The second is a configuration or credential problem. Rewriting the SQL will waste time and may hide the real issue.

## The PGSandbox error envelope

Every public PGSandbox MCP tool returns JSON text in the same envelope. Success responses contain `ok: true`, `summary`, `warnings`, `errors`, `detailHandles`, and a tool-specific `result`. Failure responses keep the same shape with `ok: false`.

The [MCP tool contract](https://pgsandbox.lvtd.dev/docs/mcp-tools/) documents the shared fields:

| Field | How an agent should use it |
| --- | --- |
| `ok` | Decide whether the tool completed. |
| `summary` | Human-readable status, useful in PR notes but not stable enough for branching. |
| `warnings` | Bounded non-fatal concerns. Preserve them in summaries. |
| `errors[].code` | Primary branch key. Examples: `syntax_error`, `readonly_violation`, `unknown_profile`. |
| `errors[].category` | Broad route. Examples: `validation`, `sql_analysis`, `timeout`, `postgres`. |
| `errors[].message` | Sanitized human detail. Read it, but do not parse it as the stable API. |
| `errors[].hint` | Recommended next action. This is the agent's first recovery instruction. |
| `errors[].sqlstate` | Postgres SQLSTATE when available. Preserve it for database-specific repair. |
| `detailHandles` | Safe follow-up pointers, often to diagnostics instead of long logs. |

PGSandbox's implementation also masks credential-bearing connection strings in errors. That matters because database errors often include enough context to tempt an agent into pasting secrets into a PR, issue, or chat transcript.

## Error codes and next actions

This table is the practical routing layer. Start here before asking a human to debug.

| Code | Category | Likely cause | Safe next action |
| --- | --- | --- | --- |
| `undefined_column` | `sql_analysis` | SQL references a missing or mis-cased column. | Call `describe_schema`, inspect identifiers, then rewrite the query. |
| `undefined_table` | `sql_analysis` | SQL references a missing relation or wrong `search_path`. | Call `describe_schema`, check schema qualification, and retry with the correct relation. |
| `syntax_error` | `sql_syntax` | SQL is malformed. | Revise SQL. Do not run `doctor`; connectivity is not the problem. |
| `constraint_violation` | `constraint_violation` | A write violated a database constraint. | Inspect the constraint, seed data, or mutation input before retrying. |
| `readonly_violation` | `readonly_violation` | Mutation was attempted with `readonly: true`, or readonly SQL included blocked controls. | Retry with `readonly: false` only when mutation is intended and sandbox-scoped. |
| `invalid_row_limit` | `validation` | `rowLimit` was negative. | Use `0` for metadata-only preview, `1` through `1000` for rows, or omit it for default `100`. |
| `single_statement_required` | `validation` | `explain_query` received multiple statements. | Pass exactly one plannable SQL statement. Split scripts into separate review steps. |
| `database_not_found` | `database_not_found` | The selector did not resolve to PGSandbox metadata. | Retry with the right `profile` or `postgresVersion`, or call `list_databases` with all-version scope. |
| `unknown_profile` | `validation` | The requested profile is not configured. | Use the `list_profiles` diagnostic handle and select a known profile. |
| `version_mismatch` | `version_mismatch` | Both `profile` and `postgresVersion` were supplied but disagree. | Omit `profile` when selecting by version, unless targeting an exact profile/version pair. |
| `postgres_version_unavailable` | `config` | No profile advertises the requested Postgres major version. | Call `list_profiles`, add a matching profile, or rerun setup for managed local discovery. |
| `local_postgres_unavailable` | `local_postgres` | Local Postgres binaries for the requested version are missing. | Call `ensure_postgres` or set `PGSANDBOX_POSTGRES_BIN_DIR` / `PGSANDBOX_POSTGRES_<MAJOR>_BIN_DIR`. |
| `postgres_auth_failed` | `postgres` | Admin or source database credentials failed. | Run `doctor` for admin profile failures; for clone source failures, check `sourceDatabaseUrl` credentials and permissions. |
| `postgres_connection_failed` | `postgres` | Host, port, socket, network, or database availability failed. | Run `doctor`, inspect local runtime status, and retry after connectivity is fixed. |
| `statement_timeout` | `timeout` | Postgres canceled the statement. | Narrow the query, add predicates, reduce work, or retry with a smaller operation. |
| `lock_timeout` | `timeout` | The statement could not acquire a needed lock. | Retry after the conflicting transaction ends or inspect active sessions. |
| `command_timeout` | `timeout` | Repo command, migration, seed, clone, or restore exceeded the workflow timeout. | Increase the bounded timeout only if the operation is expected to be long; otherwise reduce scope. |
| `restore_incompatible` | `restore_incompatible` | Clone restore encountered source/target version incompatibility. | Clone into the same or newer target major version, or produce a compatible dump. |
| `invalid_extensions` | `validation` | Requested extension names are invalid or unavailable. | Call `list_extensions` and request only extensions available on that target profile. |
| `extension_setup_required` | `validation` | Extension needs server-level setup. | Configure the Postgres profile first; do not hide server setup inside an agent retry. |

This is product-led SEO in practice: the page is not a keyword wrapper. It is a reusable operating surface for agents that need to decide what to do after a database tool fails.

## How to handle SQL repair errors

For SQL repair, use SQLSTATE when available and keep the retry local to the sandbox.

Postgres error fields include severity, SQLSTATE, primary message, detail, hint, position, schema/table/column names, and source file metadata depending on the error (https://www.postgresql.org/docs/current/protocol-error-fields.html). PGSandbox exposes the SQLSTATE field when the underlying Postgres error carries it, then maps common cases into agent-readable codes.

Use this loop:

1. Read `errors[0].code`.
2. If it is `undefined_column` or `undefined_table`, call [`describe_schema`](https://pgsandbox.lvtd.dev/docs/mcp-tools/).
3. Fix identifiers, schema qualification, or `search_path`.
4. If the query is non-trivial, inspect the plan with [`explain_query`](https://pgsandbox.lvtd.dev/blog/postgres-explain-plan-agent-sql/).
5. Run bounded proof with [`run_sql`](https://pgsandbox.lvtd.dev/blog/postgres-run-sql-bounded-results/).
6. Summarize the fix and evidence in the PR.

Do not treat `syntax_error` as an environment problem. A syntax error means the SQL needs revision. `doctor` is useful for profile and connectivity diagnostics, not for a misspelled keyword.

## How to handle readonly failures

`readonly_violation` is a decision point, not an obstacle to bypass automatically.

PGSandbox runs `run_sql` with `readonly: true` inside a read-only transaction and rolls it back afterward. PostgreSQL's transaction docs define read-only transaction characteristics through `BEGIN` and `SET TRANSACTION` (https://www.postgresql.org/docs/current/sql-begin.html, https://www.postgresql.org/docs/current/sql-set-transaction.html). Postgres itself disallows write commands in that mode.

PGSandbox adds an agent-facing policy on top:

- It rejects transaction-control escape hatches such as `BEGIN`, `COMMIT`, `ROLLBACK`, and related controls in readonly SQL.
- It rejects `RESET` and `SET SESSION` / `SET TRANSACTION` / `SET LOCAL` controls in readonly SQL.
- It may allow harmless settings Postgres permits inside the rolled-back transaction, such as `SET search_path`.
- It returns `readonly_violation` when mutation is blocked.

The safe recovery rule is simple: retry with `readonly: false` only when mutation is the task and the target is a disposable sandbox. If the agent was only trying to prove a read path, fix the SQL instead.

## How to handle profile, version, and local runtime errors

Profile and version errors should move the agent toward diagnostics, not toward SQL edits.

PGSandbox supports explicit profiles and managed local Postgres versions. If an agent requests `postgresVersion: "18"`, the managed local path can use a versioned profile such as `local-pg18`. If it supplies both `profile` and `postgresVersion`, the pair must match. A mismatch returns `version_mismatch`.

Use this recovery flow:

| Error | First diagnostic |
| --- | --- |
| `unknown_profile` | `list_profiles` |
| `postgres_version_unavailable` | `list_profiles` with discovered local versions |
| `version_mismatch` | Remove one selector or choose the exact matching pair |
| `local_postgres_unavailable` | `ensure_postgres` or `doctor` |
| `postgres_connection_failed` | `doctor`, then local runtime status |
| `postgres_auth_failed` | `doctor` for admin profile failures; source URL check for clone failures |

PostgreSQL libpq docs describe failed connection attempts as `CONNECTION_BAD`, commonly due to invalid connection parameters (https://www.postgresql.org/docs/current/libpq-connect.html). That is why connection failures should stay in the connectivity lane. Retrying a migration command will not fix a stopped local server or a stale MCP client config.

## How to handle clone and restore errors

Clone errors need one extra question: did the failure happen against the PGSandbox target or the source database?

PGSandbox distinguishes source URL failures during `clone_database`. When a source database credential, permission, host, or dump problem appears, the hint points at `sourceDatabaseUrl` instead of the admin profile. That keeps the recovery specific.

Common clone branches:

- `postgres_auth_failed`: check source credentials if the context says `sourceDatabaseUrl`; otherwise run `doctor` for the active admin profile.
- `permission_denied`: verify source database permissions, schema access, or target role ownership.
- `restore_incompatible`: compare source and target Postgres major versions.
- `command_timeout`: inspect source size, schema-only mode, excluded source extensions, and timeout budget.
- `invalid_extensions`: call `list_extensions` on the target profile before retrying.

For a coding agent, the important thing is to keep clone diagnostics secret-free. Use redacted connection strings in summaries and never paste `sourceDatabaseUrl` into the PR.

## What to put in a PR after an error

An agent should summarize error handling like a small incident record:

```text
Database proof:
- Tool: run_sql
- Target: pgsandbox-created database <id/name>
- Error code: undefined_column
- Category: sql_analysis
- SQLSTATE: 42703
- Recovery: described schema, corrected column name, reran readonly query
- Final proof: 3 bounded rows returned, truncated=false
- Cleanup: delete_database completed
```

That is enough for a reviewer to see the control flow without reading raw tool JSON. It also avoids the dangerous parts: credentials, unbounded rows, and long database error transcripts.

## FAQ

### Should an agent retry every Postgres MCP error?

No. Retry only after the error code says the failure is transient or after the hint has been followed. A `syntax_error` needs SQL revision. `postgres_auth_failed` needs configuration or credential repair. `lock_timeout` may be retryable after the conflicting transaction ends.

### Should an agent parse the human error message?

Use the message for context, but branch on `errors[].code`, `errors[].category`, and `errors[].sqlstate`. PostgreSQL recommends testing error codes rather than textual messages because text can change or be localized.

### What is the difference between `statement_timeout` and `command_timeout`?

`statement_timeout` is a Postgres-side cancellation for a SQL statement. `command_timeout` is a PGSandbox workflow timeout around a repo command, clone, restore, seed, or migration operation. The recovery is different: narrow SQL for the first, narrow or resize the workflow for the second.

### When should an agent call `doctor`?

Call `doctor` for profile health, local runtime, admin connection, version, and connectivity problems. Do not call it for normal SQL repair errors such as `syntax_error`, `undefined_column`, or `undefined_table`.

### Can readonly failures be ignored inside a sandbox?

No. A sandbox contains the blast radius, but `readonly_violation` still means the agent attempted a write during read proof. Retry with mutation enabled only when the task explicitly needs a write and the reviewer can see that decision.
