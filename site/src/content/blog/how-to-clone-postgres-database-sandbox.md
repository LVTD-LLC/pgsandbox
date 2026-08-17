---
title: "How to Clone a Postgres Database Into a Safe Sandbox"
excerpt: "Clone a Postgres database into a disposable sandbox when a coding agent needs realistic schema or data, but should not mutate the source database."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-06-30"
updatedAt: "2026-06-30"
tags: ["Postgres", "database cloning", "AI agents", "MCP", "sandboxes"]
category: "Engineering"
metaTitle: "Postgres Clone Database Into a Safe Sandbox"
metaDescription: "Clone a Postgres database into a disposable sandbox with pg_dump, pg_restore, scoped credentials, cleanup, and agent-safe data boundaries."
canonicalUrl: "https://pgsandbox.lvtd.dev/blog/how-to-clone-postgres-database-sandbox/"
heroImageUrl: ""
featured: false
sortOrder: 40
---
To clone a Postgres database safely for an AI coding agent, read from the source with `pg_dump`, restore into a newly created disposable database with `pg_restore`, run task SQL only against the sandbox role, and delete the sandbox when the task is done. The source database should be treated as read-only during the clone path.

That distinction matters. "Postgres clone database" tutorials often stop at the mechanics: dump here, restore there, fix owners if needed. For agent workflows, the mechanics are only half the job. The safer workflow also answers ownership, credential scope, cleanup, and what data shape the agent is allowed to see.

PGSandbox's clone model is deliberately narrow:

1. Create a new tracked sandbox database.
2. Create a scoped login role for that sandbox.
3. Run `pg_dump` against the source database.
4. Stream the archive into `pg_restore` connected as the sandbox role.
5. Omit ownership and privilege restoration where possible.
6. Clean up the destination sandbox if restore fails.
7. Let the agent run validation SQL only against the destination.

The result is not a database branch. It is a [database sandbox](https://pgsandbox.lvtd.dev/blog/what-is-database-sandbox/) with a copy of the source shape the agent can safely inspect, mutate, and throw away.

## When to clone a Postgres database

Clone a Postgres database when the task needs realistic database state and a blank database would hide the bug.

Good clone cases include:

- Reproducing a bug that depends on existing rows.
- Testing a migration against production-shaped tables.
- Checking generated SQL against real indexes, constraints, and extensions.
- Building a seeded demo state from a known source.
- Letting an agent validate a destructive query away from shared state.

Do not clone by default. A schema-only sandbox or small fixture is often enough. The safe rule is simple: choose the smallest data shape that proves the task.

PostgreSQL's own `pg_dump` documentation describes `pg_dump` as a utility for exporting a single database, and notes that it makes consistent exports while the database is being used concurrently without blocking readers or writers (https://www.postgresql.org/docs/current/app-pgdump.html). That makes logical dumps a practical baseline for portable cloning.

But a dump is still a copy of data. If the source contains sensitive rows, use a masked source, a reduced source, or a schema-only clone before you hand the sandbox to an agent.

## The safe clone architecture

For agent workflows, the clone should move in one direction:

1. Source database: read-only input.
2. Disposable destination: task workspace.
3. Agent SQL: runs only in the destination.
4. Cleanup: deletes only tracked destination resources.

That is the important operational boundary. The agent does not need write access to the source database to test most backend work. It needs a destination that behaves enough like the source to prove the task.

PGSandbox implements that boundary through the [`clone_database` MCP tool](https://pgsandbox.lvtd.dev/docs/mcp-tools/). The tool creates an isolated sandbox first, then restores the source into that sandbox with PostgreSQL client tools. The destination still follows the normal [PGSandbox resource model](https://pgsandbox.lvtd.dev/docs/architecture/): one database, one login role, scoped credentials, TTL metadata, and cleanup tied to resources PGSandbox created.

That gives the agent a real Postgres target without turning the source database into the agent's workspace.

## Step 1: decide the clone source

Before running any clone, decide what source is allowed.

Use this order:

1. A small development database with realistic schema and harmless rows.
2. A masked or reduced copy of production-shaped data.
3. A schema-only clone when row data is not needed.
4. A production source only with explicit human approval and a clear reason.

This is not caution for its own sake. Agents copy context into prompts, logs, transcripts, and PR notes. A clone can turn private rows into ordinary task context if the workflow is loose.

PGSandbox's docs warn not to paste production URLs into prompts when a secret input or local environment variable can provide them (https://pgsandbox.lvtd.dev/docs/mcp-tools/). Keep that rule. A source connection string is a credential, not documentation.

## Step 2: create a destination before restoring

A safe clone should restore into a database created for the task, not into a shared development database.

PostgreSQL requires superuser or `CREATEDB` privilege to create a database (https://www.postgresql.org/docs/current/sql-createdatabase.html). That is why lifecycle authority should be separated from task SQL. The admin connection can create the destination database and role; the sandbox role should own the task execution path afterward.

In a manual workflow, this usually means:

```bash
createdb target_sandbox
pg_dump --format=custom --no-owner --no-privileges "$SOURCE_URL" \
  | pg_restore --no-owner --no-privileges --single-transaction --exit-on-error \
      --dbname "$TARGET_URL"
```

That command shape is simplified. In a real script, avoid putting full connection URLs into shell history or logs. Pass secrets through environment variables, `.pgpass`, a secret store, or another local mechanism that does not end up in git or the transcript.

PGSandbox wraps the same idea in the MCP tool contract. The agent asks for a clone, the server creates a destination sandbox and scoped role, and the restore runs into that sandbox rather than into a shared database.

## Step 3: dump in a restore-friendly format

For cloning into a new task database, prefer PostgreSQL's custom dump format.

The PostgreSQL backup docs show `pg_dump -Fc dbname > filename` as the custom-format dump path and note that custom format can be restored selectively (https://www.postgresql.org/docs/current/backup-dump.html). The `pg_restore` docs describe `pg_restore` as the utility for restoring archives created by `pg_dump`, with support for selective restore and reordering archive items (https://www.postgresql.org/docs/current/app-pgrestore.html).

That pairing is useful for task sandboxes because restore behavior is explicit. You can choose schema-only clones, omit ownership and privileges, and fail the restore when an error would leave a half-valid workspace.

PGSandbox's current clone implementation uses:

- `pg_dump --format=custom`
- `pg_dump --no-owner`
- `pg_dump --no-privileges`
- optional `pg_dump --schema-only`
- `pg_restore --no-owner`
- `pg_restore --no-privileges`
- `pg_restore --single-transaction`
- `pg_restore --exit-on-error`

Those flags encode the product opinion. A cloned sandbox should be owned by the sandbox workflow where possible, not by roles copied from the source database. Restore should either complete cleanly or fail loudly.

## Step 4: restore as the sandbox role

The destination credential matters.

If the restore runs as a broad admin user and the agent later runs SQL as that same user, the clone is isolated by database name but not by authority. The safer pattern is to restore into a database whose normal task credential is a scoped sandbox role.

That is the part many clone tutorials skip because they are written for humans moving databases between environments. Agent workflows need a tighter default. The task role should be powerful enough to run the migration or validation work inside the sandbox, but not powerful enough to create and drop unrelated databases.

In PGSandbox, the admin connection performs lifecycle work. Tool calls that run user SQL connect with the generated sandbox role. The [architecture notes](https://pgsandbox.lvtd.dev/docs/architecture/) call this out directly because it is the safety boundary: lifecycle authority and task SQL are different jobs.

## Step 5: verify the clone before using it

After restore, do not immediately hand the sandbox to a long agent loop. Verify the clone is good enough for the task.

At minimum, check:

1. Expected tables exist.
2. Critical extensions exist.
3. Row counts look plausible for the chosen source.
4. The migration or bug-repro schema path is present.
5. The agent can connect with the sandbox credential.
6. The agent cannot mutate the source database.

PGSandbox exposes `describe_schema` for structured table, column, index, and extension summaries, plus `run_sql` with a capped `rowLimit` for validation queries (https://pgsandbox.lvtd.dev/docs/mcp-tools/). That is usually enough for an agent to confirm it has the right shape without dumping full tables into context.

A simple verification prompt can be enough:

```text
Use the cloned sandbox only. Describe the schema, confirm the expected tables
exist, run the migration, inspect the resulting schema, and summarize only the
validation result. Do not query or mutate the source database.
```

The point is to keep the clone as a proof environment, not a new source of unbounded data.

## Step 6: clean up the sandbox

Cleanup is part of cloning, not an afterthought.

A clone created for an agent task should have an owner, a purpose, a TTL, and a deletion path. If restore fails, the destination should be deleted or reported clearly. If the agent finishes, the sandbox should be deleted explicitly or by expiry.

PGSandbox's `clone_database` notes say that if restore fails, PGSandbox attempts to delete the newly created sandbox (https://pgsandbox.lvtd.dev/docs/mcp-tools/). The broader cleanup model is metadata-backed: `delete_database` deletes a database and role created by PGSandbox, while `cleanup_expired` deletes expired resources with dry-run support.

That scoping matters. A generic `DROP DATABASE` tool is too broad for day-to-day agent access. Cleanup should target tracked sandbox resources, not anything whose name happens to match a guess.

## Manual clone vs PGSandbox clone

Both approaches can be valid. The difference is how much safety plumbing you have to build yourself.

| Need | Manual `pg_dump` / `pg_restore` | PGSandbox clone |
| --- | --- | --- |
| Create destination | You create the database and role yourself | `clone_database` creates a tracked sandbox |
| Credential boundary | You must design it | Admin lifecycle and sandbox task role are separated |
| Ownership flags | You choose dump/restore flags | Uses no-owner and no-privileges flags |
| Schema-only option | You add `--schema-only` | `schemaOnly` input is part of the tool |
| Restore failure | You clean up manually | PGSandbox attempts destination cleanup |
| Agent workflow | You pass commands and credentials carefully | Agent uses MCP tools and scoped sandbox metadata |

If a human operator is doing a one-off database move, manual commands are fine. If a coding agent needs repeatable database proof during everyday backend work, the control plane matters more than the command itself.

## Common mistakes

The dangerous clone mistakes are usually operational, not syntactic.

### Cloning production data by habit

Production-shaped does not have to mean production rows. Start with schema-only, synthetic, masked, or reduced data unless the task truly requires more.

### Restoring into shared development state

A clone used for an agent task should not land in the same shared database other developers are using. That recreates the original problem with extra steps.

### Letting the agent keep admin credentials

The admin credential may be needed to create the destination. It should not become the task SQL credential. Use a scoped sandbox role after lifecycle setup.

### Forgetting owner and privilege behavior

Postgres dumps can carry ownership and privilege assumptions from the source environment. Use `--no-owner` and `--no-privileges` when the destination should be owned and controlled by the sandbox workflow.

### Leaving clones behind

Every clone should have a cleanup path. If the restore fails, delete the destination. If the task finishes, delete the destination. If the session is interrupted, expire it.

## A safe default prompt for agents

Use this when you want an agent to clone a Postgres database through PGSandbox without exposing more than the task needs:

```text
Clone the approved source Postgres database into a new PGSandbox sandbox.
Use schemaOnly=true unless row data is required for this task.
Set a short TTL and label the sandbox with this repo, branch, and task.
Run all validation SQL only against the cloned sandbox.
Do not print connection strings, source URLs, row data, or secrets.
After validation, delete the sandbox unless I ask you to keep it for review.
```

That prompt does not replace database permissions. It gives the agent the right operating frame while the MCP server and Postgres privileges enforce the important boundary.

## Bottom line

The safest way to clone a Postgres database for agent work is to treat the clone as a disposable task resource.

Use `pg_dump` to read the source, `pg_restore` to rebuild the destination, a scoped sandbox role for task SQL, and metadata-backed cleanup when the task is done. PGSandbox packages that pattern into a local MCP workflow so agents can prove database work against real Postgres without turning a shared database into their scratchpad.

If you are setting this up for the first time, start with the [PGSandbox install guide](https://pgsandbox.lvtd.dev/docs/install/) and the [Postgres MCP server safety checklist](https://pgsandbox.lvtd.dev/blog/postgres-mcp-server-safety-checklist/). The clone workflow is strongest when it sits inside the broader safety model: narrow tools, scoped credentials, bounded query output, and cleanup you can audit.

## FAQ

## Can I clone only the schema?

Yes. Use `pg_dump --schema-only` in a manual workflow, or set `schemaOnly` when using PGSandbox's `clone_database` tool. Schema-only clones are a good default when the agent needs migration proof but not row data.

## Should an AI agent clone production directly?

Only with explicit human approval and a strong reason. Prefer a masked, reduced, or non-production source. The agent should never need to mutate the source database as part of cloning.

## Why use `pg_dump` and `pg_restore` instead of `CREATE DATABASE ... TEMPLATE`?

`CREATE DATABASE ... TEMPLATE` can be useful inside one PostgreSQL cluster, but logical dump and restore is more portable across hosts and lets you omit ownership and privilege restoration. For agent sandboxes, that control is usually more important than copying a database as a template.

## What happens if restore fails?

In a safe workflow, restore failure should leave no ambiguous workspace. PGSandbox attempts to delete the newly created sandbox if `clone_database` restore fails, and the restore command uses `--exit-on-error` and `--single-transaction` so failures are visible.
