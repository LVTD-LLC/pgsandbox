# MCP Tool Contract

This is the proposed v0 tool surface. Names and argument shapes can change before implementation.

## `create_database`

Creates an isolated database and login role.

Inputs:

- `name_hint`: short human-readable purpose
- `ttl_minutes`: optional TTL, capped by server config
- `owner`: optional agent/session identifier
- `labels`: optional key/value metadata

Returns:

- `database_id`
- `database_name`
- `role_name`
- `expires_at`
- `connection_string`

## `delete_database`

Deletes a database and role created by this MCP.

Inputs:

- `database_id` or `database_name`

Returns:

- deletion status

## `get_connection_string`

Returns the connection string for a database created by this MCP.

Inputs:

- `database_id` or `database_name`

Returns:

- `connection_string`
- `expires_at`

## `run_sql`

Runs SQL against an experiment database.

Inputs:

- `database_id` or `database_name`
- `sql`
- `readonly`: optional boolean

Returns:

- rows for result-producing statements
- affected row count for mutations
- execution timing

## `describe_schema`

Returns tables, columns, indexes, and extensions for an experiment database.

Inputs:

- `database_id` or `database_name`

Returns:

- structured schema summary

## `list_databases`

Lists active experiment databases.

Inputs:

- optional owner/label filters

Returns:

- database metadata without full secrets

## `cleanup_expired`

Deletes expired resources.

Inputs:

- `dry_run`: optional boolean

Returns:

- resources selected
- resources deleted
- failures
