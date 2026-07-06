# MCP Tool Contract

This is the v0 tool surface. Names and argument shapes may still change before a stable release.

When a tool omits `profile`, PGSandbox uses the configured default profile. With
no explicit `PGSANDBOX_ADMIN_DATABASE_URL` or `PGSANDBOX_CONFIG`, that default
is the managed local Postgres cluster under `~/.pgsandbox/`.

Tools that accept `profile` also accept `postgresVersion`. If both are supplied,
the selected profile must carry matching `postgresVersion` metadata. On the
managed local default, requesting a version such as `"18"` starts or reuses the
isolated `local-pg18` cluster when matching local Postgres binaries are
installed.

Every MCP tool response returns JSON text in the same compact envelope. The
`Returns` sections below describe the tool-specific `result` payload inside
that envelope.

- `ok`: whether the requested operation completed
- `summary`: short human-readable outcome
- `changedObjects`: optional counts for schema changes
- `warnings`: bounded warnings; present on every envelope, even when empty
- `errors`: structured `code`, `category`, `message`, and optional `hint`
- `detailHandles`: opaque pointers agents can use in follow-up calls
- `result`: workflow-specific output when available

Creation-style tools return `connectionStringRedacted` for safe summaries and
task trackers. `get_connection_string` also returns only
`connectionStringRedacted` by default. Pass `includeCredentials: true` only
when a tool or command needs the actual credential-bearing `connectionString`,
and do not echo that sensitive value into chat, logs, PR comments, issues, or
durable datasets.

Tool failures are returned as MCP tool errors whose text content is a safe JSON
object using the same envelope shape: `ok: false`, `summary`, `warnings: []`,
`errors`, and `detailHandles`. Passwords and full connection strings are
masked. Postgres errors include `errors[].sqlstate` when it is available.
Expected failure classes use stable categories such as `sql_analysis`, `sql_syntax`,
`constraint_violation`, `readonly_violation`, `database_not_found`,
`version_mismatch`, `restore_incompatible`, `template_not_found`, and
`timeout`. Profile selection failures use `code: "unknown_profile"` with
category `validation` and a `detailHandles` entry that points to `list_profiles`,
names the invalid profile, and includes a bounded `knownProfiles` list. Version
diagnostics may also include `requestedVersion`, `sourceVersion`,
`targetVersion`, `detectedVersions`, and a `detailHandles` entry pointing to
`list_profiles` or `doctor` instead of embedding long local path traces.
Typical codes include `undefined_column`, `undefined_table`, `syntax_error`,
`permission_denied`, `lock_timeout`, `statement_timeout`,
`command_timeout`, `postgres_auth_failed`, `postgres_connection_failed`,
`unknown_profile`, `postgres_version_unavailable`,
`local_postgres_unavailable`, `invalid_ttl`, and `invalid_row_limit`.
`explain_query` multi-statement input returns `single_statement_required` with
category `validation` and a hint to pass exactly one SQL statement.

When selecting a local major version, omit `profile` and pass only
`postgresVersion`, for example `{ "postgresVersion": "18" }`. Supplying both is
reserved for intentionally targeting an exact profile/version pair, and a
mismatch returns `category: "version_mismatch"`.

## `create_database`

Creates an isolated database and login role.

Inputs:

- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version, for example `"16"`
- `nameHint`: short human-readable purpose
- `ttlMinutes`: optional positive TTL in minutes, capped by server config.
  Omit it to use the profile default. `0`, negative values, and values above
  `maxTtlMinutes` return `code: "invalid_ttl"`.
- `owner`: optional agent/session identifier
- `labels`: optional key/value metadata

Returns:

- `databaseId`
- `databaseName`
- `roleName`
- `expiresAt`
- `connectionStringRedacted`: safe display value for logs, task trackers, and
  summaries

## `list_profiles`

Lists configured profiles and discoverable local Postgres installations.

Inputs:

- `includeDiscoveredLocal`: optional boolean, defaults to true

Returns:

- `serverVersion`
- `toolCount`
- `restartRequiredAfterSetupNote`: advisory text. This is not live restart
  state; it reminds clients to restart after setup or upgrades because MCP
  clients cache tool metadata.
- `availablePostgresVersions`
- `hints`
- `profiles`: profile summaries with `name`, `postgresVersion`, `managedLocal`,
  `port`, masked `adminUrl`, and `source`. `port` is present when the profile
  has a concrete admin URL; discoverable local versions that have not been
  started yet report `adminUrl: "(managed local; starts on demand)"`.

Use `includeDiscoveredLocal: true` before requesting a `postgresVersion`. The
server does not download Postgres; the requested major must be installed locally
or supplied through `PGSANDBOX_POSTGRES_BIN_DIR` or
`PGSANDBOX_POSTGRES_<MAJOR>_BIN_DIR`.

## `doctor`

Returns MCP-safe version, profile health, and redacted diagnostics without
mutating sandboxes.

Inputs:

- `postgresVersion`: optional Postgres major version to include in config
  resolution and local-runtime checks

Returns:

- `ok`: whether diagnostics passed
- `serverVersion`
- `toolCount`
- `availablePostgresVersions`: discovered local Postgres majors, empty when
  none are found
- `lines`: bounded human-readable diagnostic lines with passwords masked

Agents should call this when troubleshooting connectivity, version discovery,
or MCP setup problems. It is the MCP equivalent of `pgsandbox-mcp doctor`.

## `clone_database`

Creates an isolated sandbox database and restores a dump from an existing
Postgres source into it.

The source database is read with `pg_dump`. The target sandbox is restored with
`pg_restore` using the generated sandbox role, not the admin role. Ownership
and privileges are omitted during dump/restore so cloned objects belong to the
sandbox role where possible.

Inputs:

- `profile`: optional target Postgres profile name
- `postgresVersion`: optional target Postgres major version
- `sourceDatabaseUrl`: source Postgres connection string
- `nameHint`: short human-readable purpose
- `ttlMinutes`: optional positive TTL in minutes, capped by server config.
  Omit it to use the profile default. `0`, negative values, and values above
  `maxTtlMinutes` return `code: "invalid_ttl"`.
- `owner`: optional agent/session identifier
- `labels`: optional key/value metadata
- `schemaOnly`: optional boolean to clone schema without table data

Returns:

- `databaseId`
- `profile`
- `databaseName`
- `roleName`
- `expiresAt`
- `connectionStringRedacted`
- `source`: currently `external`
- `schemaOnly`

Notes:

- Requires `pg_dump` and `pg_restore` on `PATH` for this tool only.
- If restore fails, PGSandbox attempts to delete the newly created sandbox.
- Do not paste production URLs into prompts when a secret input or local
  environment variable can provide them.

## `delete_database`

Deletes a database and role created by this MCP.

Inputs:

- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version
- `databaseId` or `databaseName`

Returns:

- deletion status

## `get_connection_string`

Returns the redacted connection string for a database created by this MCP. The
raw credential-bearing `connectionString` is sensitive and is only returned
when `includeCredentials` is true.

Inputs:

- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version
- `databaseId` or `databaseName`
- `includeCredentials`: optional boolean, defaults to false. When true, the
  response includes raw `connectionString` with sandbox role credentials.

Returns:

- `connectionStringRedacted`: safe display value for logs, task trackers, and
  summaries
- `connectionString`: present only when `includeCredentials` is true; sensitive
  credential-bearing value for direct database clients and commands
- `expiresAt`

## `run_sql`

Runs SQL against an experiment database.

Inputs:

- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version
- `databaseId` or `databaseName`
- `sql`
- `readonly`: optional boolean
- `rowLimit`: optional max row count. Omit it to use the default of 100,
  pass `0` for a zero-row preview, or pass `1` through `1000` to return rows.
  Negative values return `code: "invalid_row_limit"` and values above `1000`
  are capped at `1000`.

Returns:

- `rows`: rows from the last row-returning statement, or an empty array when no
  statement returned rows
- `resultSets`: ordered per-statement results. Each entry includes 1-based
  `statementIndex`, `rows`, `returnedRowCount`, `affectedRowCount`,
  `totalRowCountKnown`, and `truncated`. The row limit applies independently to
  each row-returning result set.
- affected row count for mutations
- `returnedRowCount`: number of rows included in `rows`
- `affectedRowCount`: affected rows for DML/DDL command tags when applicable
- `totalRowCountKnown`: whether the total row count is known without inference
- `truncated`: whether `rows` was bounded by `rowLimit`
- execution timing

Typed result rows serialize common scalar and array values to JSON. `int8`
values, including `count(*)` aggregate results, and `numeric` values are
returned as strings to preserve precision. `timestamp`, `timestamptz`, and
`date` values are returned as strings. `json` and `jsonb` values are returned
as nested JSON. Common Postgres arrays such as `text[]`, integer arrays,
`uuid[]`, `jsonb[]`, and `timestamptz[]` return JSON arrays with SQL `NULL`
elements preserved as JSON `null`; `int8[]` elements follow the same string
serialization rule. Multi-statement SQL is split into ordered statements and
each row-returning statement is serialized with the same typed rules as a
single-statement query. Unsupported non-null Postgres result types return a
structured object with `unsupportedPostgresType` and a cast-to-text `hint`;
unsupported SQL `NULL` values remain JSON `null`. With `readonly: true`,
mutating statements are blocked by a read-only transaction; readonly violations
are wrapped with an MCP-level message that names the attempted statement while
preserving database detail.

## `describe_schema`

Returns relation, column, constraint, index, view, and extension metadata for an
experiment database.

Inputs:

- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version
- `databaseId` or `databaseName`

Returns:

- structured schema summary with compact canonical camelCase keys such as
  `tableName`, `tableSchema`, `columnName`, `dataType`, and `isNullable`.
- `relationCounts`: split counts for `tables`, `partitionedTables`, `views`,
  `materializedViews`, `foreignTables`, and `other`.
- `relations`: all table-like, view-like, and foreign-table relations. Every
  relation includes `relationKind`; views and materialized views also include a
  `definition` string from Postgres.
- `tables`: regular table relations only, with `relationKind: "table"`.
- `partitionedTables`: partitioned table relations only, with
  `relationKind: "partitioned_table"`.
- `foreignTables`: foreign table relations only, with
  `relationKind: "foreign_table"`.
- `views`: view relations only, with `relationKind: "view"` and `definition`.
- `materializedViews`: materialized view relations only, with
  `relationKind: "materialized_view"` and `definition`.
- `columns`: includes `columnDefault`, `generatedKind`, and
  `generationExpression` when Postgres exposes them.
- `constraints`: primary key, unique, foreign key, check, exclusion, and
  not-null constraints with semantic `constraintType` values such as
  `primary_key`, `foreign_key`, and `not_null`; includes readable definitions
  and FK actions when applicable.
- `indexes` and `extensions`.

## `schema_digest`

Returns a compact, checksumed schema summary for an experiment database. The
checksum is based on schema objects, not the sandbox database id or name, so it
can be compared across sandboxes.

Inputs:

- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version
- `databaseId` or `databaseName`

Returns:

- `databaseId`
- `databaseName`
- `digestVersion`
- `checksum`
- `relationCounts`: split counts for tables, partitioned tables, views,
  materialized views, foreign tables, and other relation kinds.
- object counts for tables, columns, constraints, indexes, and extensions
- compact tables with relation kind, column type/nullability/default/generated
  metadata, constraint definition hashes, index definition hashes, and view
  definition hashes. Constraint types are semantic values such as
  `primary_key`, `foreign_key`, and `not_null`, not raw PostgreSQL catalog
  codes.
- extensions with versions

## `schema_diff`

Compares a prior `schema_digest` response with the current schema of an
experiment database.

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`
- `baseDigest`: a previous `schema_digest` result object or the full MCP
  envelope that contains it under `result`. A JSON string containing either
  shape is also accepted for agent workflows that pass tool output through
  string-only storage. A checksum string alone is not enough to compute a diff;
  checksum-only input returns `code: "invalid_base_digest"` with a hint to pass
  the full object or use schema snapshots.

Example:

```json
{
  "databaseId": "6d4b...",
  "baseDigest": {
    "databaseId": "6d4b...",
    "databaseName": "pgsandbox_app_abc12345",
    "digestVersion": 3,
    "checksum": "...",
    "relationCounts": {
      "tables": 1,
      "partitionedTables": 0,
      "views": 0,
      "materializedViews": 0,
      "foreignTables": 0,
      "other": 0
    },
    "columnCount": 3,
    "constraintCount": 1,
    "indexCount": 1,
    "extensionCount": 1,
    "tables": [],
    "extensions": []
  }
}
```

Returns:

- `beforeChecksum`
- `afterChecksum`
- `changed`
- added and removed tables
- changed tables with added, removed, or changed columns, indexes, and
  constraints, plus `viewDefinitionChanged` for view body changes
- added, removed, or changed extensions

## `explain_query`

Returns `EXPLAIN (FORMAT JSON)` for one SQL statement against an experiment
database, plus a compact summary of node types, relations, cost, and estimated
rows. The tool does not use `ANALYZE`; it rejects multi-statement SQL,
transaction/session controls, and statements outside SELECT/WITH/VALUES/TABLE
or DML forms that Postgres can plan without executing.
Multi-statement SQL fails with `code: "single_statement_required"` and
`category: "validation"`; trim the input to exactly one statement before
retrying.

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`
- `sql`

Returns:

- `databaseId`
- `databaseName`
- `summary`
- `plan`

## Schema Snapshots

Schema snapshots are explicit named checkpoints stored under PG Sandbox's local
state directory. They are not automatic truth; create a new snapshot whenever a
new before-state matters.

Use the schema snapshot tools together for before/after migration review,
schema diff workflows, rollback comparison, drift detection, and stored schema
baselines. The related tools are `create_schema_snapshot`,
`list_schema_snapshots`, `diff_schema_snapshot`, and `delete_schema_snapshot`.

### `create_schema_snapshot`

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`
- `snapshotName`: local artifact name
- `notes`: optional short notes

Returns snapshot metadata, object counts, and a detail handle.
Object counts split tables, views, materialized views, foreign tables,
constraints, indexes, columns, and extensions so snapshot counts line up with
`schema_digest` relation counts.

### `list_schema_snapshots`

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`

Returns snapshot summaries for the selected PGSandbox-owned database.

### `diff_schema_snapshot`

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`
- `snapshotName`

Returns the saved snapshot compared to the current sandbox schema.

### `delete_schema_snapshot`

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`
- `snapshotName`

Deletes the local snapshot artifact.

## Repo Workflow Tools

These tools are repo-aware but conservative. They do not rewrite application
settings permanently and they execute the exact argv array supplied by the
caller without an implicit shell. Database access is injected through
`DATABASE_URL`, `PGSANDBOX_DATABASE_URL`, and libpq-style `PG*` environment
variables. Use `run_sql` for direct SQL, `run_repo_command` for an explicit
repo command, and `validate_schema_change` when a before/after schema diff is
needed. Django detection remains a convenience preset, not a requirement.

### `prepare_for_repo`

Writes a secret-free `.pgsandbox/project.json` with generic workflow metadata
and optional explicit command argv arrays. It does not detect or assume an
application framework. If `postgresVersion` is omitted, PGSandbox checks
existing `.pgsandbox/project.json`, then Compose/devcontainer image references
such as `postgres:16` or `postgis/postgis:16-3.4`, and records the inferred
version when found.

Inputs:

- `repoPath`
- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version
- `databaseId` or `databaseName`: optional sandbox to report as the masked target
- `migrationCommand`: optional argv array for the repo migration workflow
- `seedCommand`: optional argv array for the repo seed workflow

### `run_repo_command`

Runs an explicit or configured repo schema-change command against a selected
sandbox.

Inputs:

- `repoPath`
- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version; defaults from
  `.pgsandbox/project.json` when present
- `databaseId` or `databaseName`
- `command`: optional argv array; defaults to `.pgsandbox/project.json`
- `timeoutSeconds`: optional timeout, capped by the server

The command runs with `repoPath` as current directory. The argv array must be
short, non-empty, free of NUL/newline characters, and is executed directly
without shell expansion or indirect launchers.

Shell wrappers and command launchers such as `["bash", "-lc", "..."]`,
`["sh", "-c", "..."]`, `env`, `sudo`, and `nsenter` are intentionally rejected
with `code: "unsafe_command"`. Pass direct argv instead:

```json
["npm", "run", "migrate"]
```

```json
["psql", "-v", "ON_ERROR_STOP=1", "-f", "migrations/schema.sql"]
```

```json
["psql", "-Atc", "SELECT current_database(), current_user"]
```

For multi-step workflows, prefer a repo/package script that can be invoked
directly, such as `["./scripts/seed.sh"]` after `chmod +x scripts/seed.sh` if
needed, or split the work into separate tool calls instead of sending a shell
snippet.

Returns bounded command output with `databaseId`, `databaseName`, `command`,
`elapsedMs`, `exitCode`, `timedOut`, `stdout`, `stderr`,
`stdoutTruncated`, and `stderrTruncated`. When the command exceeds
`timeoutSeconds`, the workflow returns `ok: false` with
`code: "command_timeout"` and `category: "timeout"`; `result.exitCode`
remains `null`, `result.timedOut` is `true`, and `stderr` may include the
human-readable timeout line.

### `validate_schema_change`

Captures a before schema digest, runs an explicit or configured repo
schema-change command against a fresh or selected sandbox, captures the after
digest, and returns a compact schema diff plus bounded command output.

Inputs:

- `repoPath`
- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version; defaults from explicit
  input, `.pgsandbox/project.json`, or repo inference when creating a sandbox
- `databaseId` or `databaseName`: optional; omitted creates a fresh sandbox
- `command`: optional argv array; defaults to `.pgsandbox/project.json`
- `timeoutSeconds`
- `nameHint`, `ttlMinutes`, `owner`, `labels`: used when creating a fresh
  sandbox. `ttlMinutes` follows the same positive-only validation as
  `create_database`.

If `databaseId`/`databaseName` is omitted, this tool creates a sandbox and sets
`result.createdSandbox: true`. Failed validations delete that auto-created
sandbox when cleanup succeeds; successful validations return the created
sandbox id so callers can inspect it or delete it explicitly.

Command timeouts use the same `command_timeout` / `timeout` structured error
as `run_repo_command`, with `result.exitCode: null` and
`result.timedOut: true` in the bounded command output.

### `seed_database`

Runs only an explicit configured seed command against a selected sandbox. It
does not auto-discover or auto-run repo scripts.

Seed commands follow the same no-shell rule as other workflow commands. Shell
wrappers such as `["bash", "scripts/seed.sh"]` or `["sh", "-c", "..."]` fail
with `code: "unsafe_command"`. To run a repo seed script, make it executable if
needed and pass it directly:

```json
["./scripts/seed.sh"]
```

Inputs:

- `repoPath`
- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version; defaults from
  `.pgsandbox/project.json` when present
- `databaseId` or `databaseName`
- `command`: optional argv array; defaults to `.pgsandbox/project.json` `seedCommand`
- `timeoutSeconds`

Command timeouts use the same `command_timeout` / `timeout` structured error
as `run_repo_command`, with `result.exitCode: null` and
`result.timedOut: true` in the bounded command output.

## Template Tools

Templates are local artifacts under PG Sandbox's managed state directory. They
are created only from PGSandbox-owned databases and restored into newly tracked
PGSandbox-owned sandboxes. They use `pg_dump`/`pg_restore` and are not
copy-on-write forks, hosted snapshots, or production-data workflows.

Use the template tools together for reusable seeded sandbox workflows,
regression fixtures, repeatable test states, and local template restore loops.
The related tools are `create_template_from_sandbox`,
`create_sandbox_from_template`, `list_templates`, and `delete_template`.

### `create_template_from_sandbox`

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`
- `templateName`
- `createdBy`: optional actor label
- `notes`: optional notes

Returns metadata including source sandbox id, created time, owner, Postgres
version, size estimate, notes, and a privacy warning.

### `create_sandbox_from_template`

Inputs:

- `profile`: optional Postgres profile name
- `templateName`
- `nameHint`
- `ttlMinutes`: optional positive TTL in minutes; omit it to use the profile
  default
- `owner`
- `labels`

Returns the new sandbox metadata under `result` with
`connectionStringRedacted`. Call `get_connection_string` with the returned
`databaseId` and `includeCredentials: true` only when the full connection string
is explicitly needed.

### `list_templates`

Inputs:

- `profile`: optional Postgres profile name

Returns local template metadata for the selected profile.

### `delete_template`

Inputs:

- `profile`: optional Postgres profile name
- `templateName`

Deletes the local template dump and metadata.

## `list_databases`

Lists active experiment databases.

Inputs:

- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version. Use `"*"` to list across
  configured profiles and running managed-local version profiles.
- `includeAllVersions`: optional boolean. When true, lists across configured
  profiles and running managed-local version profiles. Do not combine with
  `profile`.
- `owner`: optional owner filter

Returns:

- `scope`: `"profile"` or `"allVersions"`
- `profiles`: profiles included in the listing
- `databases`: database metadata without full secrets, using camelCase keys
  such as `databaseId`, `databaseName`, `roleName`, `profile`, `createdAt`, and
  `expiresAt`.
- `truncated`: whether more matching records exist beyond the returned page
- `failures`: profile-level failures for all-version listings. Each entry has
  `profile`, `category: "profile_unavailable"`, and a safe `message`.

## `cleanup_expired`

Deletes expired resources.

Inputs:

- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version. Use `"*"` to clean up
  across configured profiles and running managed-local version profiles.
- `includeAllVersions`: optional boolean. When true, cleans up across configured
  profiles and running managed-local version profiles. Do not combine with
  `profile`.
- `dryRun`: optional boolean
- `owner`: optional owner filter. When supplied, cleanup only selects expired
  sandboxes whose stored owner exactly matches this value.
- `labels`: optional label filter object. When supplied, cleanup only selects
  expired sandboxes whose stored labels contain every provided key/value pair.
  Sandboxes may have additional labels. When `owner` and `labels` are both
  supplied, both filters must match.

Returns:

- `scope`: `"profile"` or `"allVersions"`
- `profile`: selected profile for scoped cleanup
- `profiles`: profiles included in the cleanup
- `remainingProfiles`: other known profiles when cleanup was scoped to one
  profile, so agents know whether another cleanup call may be needed
- `dryRun`: whether the call only selected candidates
- `filters`: applied cleanup filters with `owner` and `labels`
- `selected`: resources selected when `dryRun` is true
- `deleted`: deleted database ids when `dryRun` is false
- `failures`: deletion or profile-level failures. All-version cleanup reports
  profile-level failures with `profile`,
  `category: "profile_unavailable"`, and a safe `message` while continuing with
  other profiles.

## Stable Agent Contract

`databaseId` and unscoped `databaseName` are globally resolvable by id/name-only
calls when the server can search configured profiles and running managed-local
profiles. If a profile cannot be searched or the sandbox is not found, the
error uses `category: "database_not_found"` and tells the caller to retry with
`profile`/`postgresVersion` or call `list_databases` with
`includeAllVersions=true`. If a name matches multiple profiles, retry with the
profile or Postgres version to disambiguate.

Unversioned `list_databases` and `cleanup_expired` are scoped to the selected
default profile. Use `includeAllVersions=true` or `postgresVersion: "*"` when an
agent needs a cross-version view or cleanup pass.

Major-only version strings such as `"16"`, `"17"`, and `"18"` are canonical.
Patch versions such as `"18.4"` are normalized to the major version for local
profile selection.

Clone downgrades are not supported by default. PGSandbox checks the source and
target Postgres majors before creating the target sandbox and returns
`category: "restore_incompatible"` when the source major is newer than the
target. The error includes `sourceVersion` and `targetVersion`.
