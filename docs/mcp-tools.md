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

Workflow-oriented tools return a compact result envelope:

- `ok`: whether the requested workflow completed
- `summary`: short human-readable outcome
- `changedObjects`: optional counts for schema changes
- `warnings`: bounded warnings
- `errors`: structured `code`, `message`, and optional `hint`
- `detailHandles`: opaque pointers agents can use in follow-up calls
- `result`: workflow-specific output when available

## `create_database`

Creates an isolated database and login role.

Inputs:

- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version, for example `"16"`
- `nameHint`: short human-readable purpose
- `ttlMinutes`: optional TTL, capped by server config
- `owner`: optional agent/session identifier
- `labels`: optional key/value metadata

Returns:

- `databaseId`
- `databaseName`
- `roleName`
- `expiresAt`
- `connectionString`

## `list_profiles`

Lists configured profiles and discoverable local Postgres installations.

Inputs:

- `includeDiscoveredLocal`: optional boolean, defaults to true

Returns:

- `profiles`: profile summaries with `name`, `postgresVersion`, `managedLocal`,
  masked `adminUrl`, and `source`

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
- `ttlMinutes`: optional TTL, capped by server config
- `owner`: optional agent/session identifier
- `labels`: optional key/value metadata
- `schemaOnly`: optional boolean to clone schema without table data

Returns:

- `databaseId`
- `profile`
- `databaseName`
- `roleName`
- `expiresAt`
- `connectionString`
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
- `databaseId` or `databaseName`

Returns:

- deletion status

## `get_connection_string`

Returns the connection string for a database created by this MCP.

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`

Returns:

- `connectionString`
- `expiresAt`

## `run_sql`

Runs SQL against an experiment database.

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`
- `sql`
- `readonly`: optional boolean
- `rowLimit`: optional max row count, capped at 1000

Returns:

- rows for result-producing statements
- affected row count for mutations
- execution timing

## `describe_schema`

Returns tables, columns, indexes, and extensions for an experiment database.

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`

Returns:

- structured schema summary

## `schema_digest`

Returns a compact, checksumed schema summary for an experiment database. The
checksum is based on schema objects, not the sandbox database id or name, so it
can be compared across sandboxes.

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`

Returns:

- `databaseId`
- `databaseName`
- `digestVersion`
- `checksum`
- object counts for tables, columns, indexes, and extensions
- compact tables with column type/nullability and index definition hashes
- extensions with versions

## `schema_diff`

Compares a prior `schema_digest` response with the current schema of an
experiment database.

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`
- `baseDigest`: a previous `schema_digest` response

Returns:

- `beforeChecksum`
- `afterChecksum`
- `changed`
- added and removed tables
- changed tables with added, removed, or changed columns and indexes
- added, removed, or changed extensions

## `explain_query`

Returns `EXPLAIN (FORMAT JSON)` for one SQL statement against an experiment
database, plus a compact summary of node types, relations, cost, and estimated
rows. The tool does not use `ANALYZE`; it rejects multi-statement SQL,
transaction/session controls, and statements outside SELECT/WITH/VALUES/TABLE
or DML forms that Postgres can plan without executing.

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

### `create_schema_snapshot`

Inputs:

- `profile`: optional Postgres profile name
- `databaseId` or `databaseName`
- `snapshotName`: local artifact name
- `notes`: optional short notes

Returns snapshot metadata, object counts, and a detail handle.

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

## Django Workflow Tools

These tools are repo-aware but conservative. They do not rewrite application
settings permanently and they run commands without a shell. Database access is
injected through `DATABASE_URL`, `PGSANDBOX_DATABASE_URL`, and libpq-style
`PG*` environment variables.

### `prepare_for_repo`

Detects a Django repo from `manage.py` and settings patterns, then writes a
secret-free `.pgsandbox/project.json` with a default Django migration command.
If detection is uncertain, the tool returns `ok: false` with an action-needed
message instead of guessing. If `postgresVersion` is omitted, PGSandbox checks
existing `.pgsandbox/project.json`, then Compose/devcontainer image references
such as `postgres:16` or `postgis/postgis:16-3.4`, and records the inferred
version when found.

Inputs:

- `repoPath`
- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version
- `databaseId` or `databaseName`: optional sandbox to report as the masked target

### `run_migrations`

Runs only an explicit Django `migrate` command against a selected sandbox.

Inputs:

- `repoPath`
- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version; defaults from
  `.pgsandbox/project.json` when present
- `databaseId` or `databaseName`
- `command`: optional argv array; defaults to `.pgsandbox/project.json`
- `timeoutSeconds`: optional timeout, capped by the server

### `validate_migration`

Captures a before schema digest, runs the Django migration command against a
fresh or selected sandbox, captures the after digest, and returns a compact
schema diff plus bounded command output.

Inputs:

- `repoPath`
- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version; defaults from explicit
  input, `.pgsandbox/project.json`, or repo inference when creating a sandbox
- `databaseId` or `databaseName`: optional; omitted creates a fresh sandbox
- `command`: optional argv array; defaults to `.pgsandbox/project.json`
- `timeoutSeconds`
- `nameHint`, `ttlMinutes`, `owner`, `labels`: used when creating a fresh sandbox

### `seed_database`

Runs only an explicit configured seed command against a selected sandbox. It
does not auto-discover or auto-run repo scripts.

Inputs:

- `repoPath`
- `profile`: optional Postgres profile name
- `postgresVersion`: optional Postgres major version; defaults from
  `.pgsandbox/project.json` when present
- `databaseId` or `databaseName`
- `command`: optional argv array; defaults to `.pgsandbox/project.json` `seedCommand`
- `timeoutSeconds`

## Template Tools

Templates are local artifacts under PG Sandbox's managed state directory. They
are created only from PGSandbox-owned databases and restored into newly tracked
PGSandbox-owned sandboxes. They use `pg_dump`/`pg_restore` and are not
copy-on-write forks, hosted snapshots, or production-data workflows.

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
- `ttlMinutes`
- `owner`
- `labels`

Returns the new sandbox metadata and connection string.

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
- `owner`: optional owner filter

Returns:

- `databases`: database metadata without full secrets
- `truncated`: whether more matching records exist beyond the returned page

## `cleanup_expired`

Deletes expired resources.

Inputs:

- `profile`: optional Postgres profile name
- `dryRun`: optional boolean

Returns:

- resources selected
- resources deleted
- failures
