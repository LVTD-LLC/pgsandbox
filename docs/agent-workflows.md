# Agent Workflows

Use these workflows when an agent needs to validate Postgres behavior without
assuming a framework.

`ttlMinutes` values in these examples are positive minutes. Omit the field to
use the profile default; `0` and negative values are rejected with
`invalid_ttl`.

## Direct SQL Schema Change

1. Create a sandbox.

```json
{
  "tool": "create_database",
  "arguments": {
    "nameHint": "billing status check",
    "ttlMinutes": 45,
    "owner": "agent-session"
  }
}
```

2. Apply SQL directly.

```json
{
  "tool": "run_sql",
  "arguments": {
    "databaseId": "<databaseId>",
    "sql": "CREATE TABLE accounts(id serial PRIMARY KEY, email text UNIQUE);"
  }
}
```

3. Inspect the schema.

```json
{
  "tool": "describe_schema",
  "arguments": {
    "databaseId": "<databaseId>"
  }
}
```

4. Delete the sandbox when finished.

```json
{
  "tool": "delete_database",
  "arguments": {
    "databaseId": "<databaseId>"
  }
}
```

## Before/After Diff

Use the full `schema_digest` response as `baseDigest`; a checksum string alone
cannot compute a diff.

```json
{
  "tool": "schema_digest",
  "arguments": {
    "databaseId": "<databaseId>"
  }
}
```

```json
{
  "tool": "schema_diff",
  "arguments": {
    "databaseId": "<databaseId>",
    "baseDigest": {
      "databaseId": "<databaseId>",
      "databaseName": "<databaseName>",
      "digestVersion": 3,
      "checksum": "<checksum>",
      "relationCounts": {
        "tables": 0,
        "partitionedTables": 0,
        "views": 0,
        "materializedViews": 0,
        "foreignTables": 0,
        "other": 0
      },
      "columnCount": 0,
      "constraintCount": 0,
      "indexCount": 0,
      "extensionCount": 0,
      "tables": [],
      "extensions": []
    }
  }
}
```

For a compact stored baseline, prefer snapshots:

```json
{
  "tool": "create_schema_snapshot",
  "arguments": {
    "databaseId": "<databaseId>",
    "snapshotName": "before_change",
    "notes": "baseline before repo command"
  }
}
```

```json
{
  "tool": "diff_schema_snapshot",
  "arguments": {
    "databaseId": "<databaseId>",
    "snapshotName": "before_change"
  }
}
```

## Row-Limited Reads

`run_sql` returns explicit row metadata:

```json
{
  "tool": "run_sql",
  "arguments": {
    "databaseId": "<databaseId>",
    "readonly": true,
    "rowLimit": 2,
    "sql": "SELECT * FROM accounts ORDER BY id"
  }
}
```

Use `returnedRowCount`, `affectedRowCount`, `totalRowCountKnown`, and
`truncated` for result-size and mutation checks.

## Repo Command Validation

Use `run_repo_command` when the repo already has a script. Use
`validate_schema_change` when the agent needs the schema diff.

```json
{
  "tool": "validate_schema_change",
  "arguments": {
    "repoPath": "/absolute/path/to/repo",
    "nameHint": "validate repo schema change",
    "ttlMinutes": 45,
    "owner": "agent-session",
    "command": ["npm", "run", "migrate"]
  }
}
```

Commands run with `repoPath` as current directory and receive `DATABASE_URL`,
`PGSANDBOX_DATABASE_URL`, and libpq `PG*` variables for the sandbox. PGSandbox
does not add an implicit shell, and it rejects shell wrappers and indirect
launchers such as `["bash", "-lc", "..."]`, `["sh", "-c", "..."]`, `env`, and
`sudo`. Pass direct argv such as `["npm", "run", "migrate"]`,
`["psql", "-v", "ON_ERROR_STOP=1", "-f", "migrations/schema.sql"]`, or
`["psql", "-Atc", "SELECT current_database(), current_user"]`. For multi-step
workflows, prefer a repo/package script that can be invoked directly, or split
the work into separate tool calls.

## Templates And Clone

Create a reusable local template from a sandbox:

```json
{
  "tool": "create_template_from_sandbox",
  "arguments": {
    "databaseId": "<databaseId>",
    "templateName": "seeded_accounts",
    "createdBy": "agent-session"
  }
}
```

Restore it into a new sandbox:

```json
{
  "tool": "create_sandbox_from_template",
  "arguments": {
    "templateName": "seeded_accounts",
    "nameHint": "replay bug",
    "ttlMinutes": 45,
    "owner": "agent-session"
  }
}
```

For external sources, use `clone_database` with `schemaOnly` when data is not
needed. Do not paste production URLs into issue trackers, Rowset rows, or logs.

## Cleanup And Diagnostics

List or clean up one profile by default:

```json
{
  "tool": "list_databases",
  "arguments": {
    "owner": "agent-session"
  }
}
```

Use all-version scope deliberately:

```json
{
  "tool": "cleanup_expired",
  "arguments": {
    "includeAllVersions": true,
    "dryRun": true
  }
}
```

Use MCP `doctor` when shell access is unavailable:

```json
{
  "tool": "doctor",
  "arguments": {}
}
```

## Error Handling

Branch on stable `error.code` and `error.category`, not prose. Common SQL
repair cases include:

- `undefined_column` or `undefined_table`: call `describe_schema` or check
  identifiers.
- `syntax_error`: revise SQL; `doctor` is not the first recovery step.
- `readonly_violation`: retry with `readonly: false` only when mutation is
  intended.
- `constraint_violation`: inspect the constraint and adjust input.
- `statement_timeout` or `lock_timeout`: reduce scope or retry after the
  conflicting operation ends.

Creation tools return `connectionStringRedacted` for summaries, task trackers,
and logs. Call `get_connection_string` only when a command or database client
needs the actual credential, and do not echo that full value into chat, logs, or
durable datasets.
