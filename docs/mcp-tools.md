# MCP Tool Contract

This is the v0 tool surface. Names and argument shapes may still change before a stable release.

## `create_database`

Creates an isolated database and login role.

Inputs:

- `profile`: optional Postgres profile name
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
